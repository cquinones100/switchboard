//! Every picture reaches a terminal the one way src/mux/graphics.rs
//! chooses: no other file in src/ writes out kitty's graphics commands or
//! a sixel in its source (or any DCS, tmux's passthrough among them), so a
//! picture added later doesn't speak one protocol and miss the other; and
//! with Settings' "Image protocol" on sixel, the server's own pictures (the
//! image atoms, the picture shown full size) reach a terminal that doesn't
//! speak kitty's as sixel, never as kitty's, which Windows Terminal doesn't
//! draw, while one that does keeps kitty's. The emulator the tests read the
//! screen through draws neither, so these read the bytes.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::common::{self, TestServer};

/// The files in src/ that may write kitty's graphics commands or a DCS,
/// and why: the pipeline's two writers, and what reads them.
const ALLOWED: &[(&str, &str)] = &[
    ("src/mux/graphics.rs", "writes kitty's commands"),
    ("src/mux/sixel/encode.rs", "writes a sixel"),
    ("src/mux/sixel/kitty.rs", "reads the commands a pane gives"),
    (
        "src/mux/passthrough.rs",
        "picks the commands out of a pane's output",
    ),
    (
        "src/ui/headless.rs",
        "reads what the server sent, as a terminal would",
    ),
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A file's code without its tests: each `#[cfg(test)]` module written in
/// it left out (rustfmt's layout: the module's lines up to the `}` that
/// closes it at the start of a line), and the files of those declared in
/// it (`#[cfg(test)] mod tests;`) added to `test_files`.
fn without_tests(file: &Path, text: &str, test_files: &mut HashSet<PathBuf>) -> String {
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let dir = file.parent().unwrap();
    let children = if matches!(stem, "mod" | "lib" | "main") {
        dir.to_path_buf()
    } else {
        dir.join(stem)
    };
    let mut out = String::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() == "#[cfg(test)]" {
            let next = lines.peek().map(|l| l.trim()).unwrap_or("");
            let declared = next
                .trim_start_matches("pub(crate) ")
                .trim_start_matches("pub ")
                .strip_prefix("mod ");
            if let Some(name) = declared.and_then(|d| d.strip_suffix(';')) {
                test_files.insert(children.join(format!("{name}.rs")));
                test_files.insert(children.join(name).join("mod.rs"));
                lines.next();
                continue;
            }
            if declared.is_some_and(|d| d.ends_with('{')) {
                for inner in lines.by_ref() {
                    if inner == "}" {
                        break;
                    }
                }
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// What a file's code writes that only the pipeline may, as the source
/// writes it out: kitty's graphics command introducer (`ESC _ G`) or a DCS
/// (`ESC P`), ESC in any of Rust's escapes for it (`\x1b`, `\u{1b}` and
/// its padded forms) or as the byte 0x1b in an array (`[0x1b, b'P']`). A
/// check of the source, not of what runs: an ESC built some other way
/// (`concat!`, a constant) passes it, so a new writer is still a reviewer's
/// to catch; this catches the ordinary ways of writing one.
fn escapes(code: &str) -> Vec<String> {
    let patterns = [
        (r"\\x1[bB]_G", "ESC _ G"),
        (r"\\u\{0*1[bB]\}_G", "ESC _ G"),
        (r"0x1[bB]\s*,\s*b'_'\s*,\s*b'G'", "ESC _ G"),
        (r"\\x1[bB]P", "ESC P"),
        (r"\\u\{0*1[bB]\}P", "ESC P"),
        (r"0x1[bB]\s*,\s*b'P'", "ESC P"),
    ];
    let mut found: Vec<String> = patterns
        .iter()
        .filter(|(p, _)| regex::Regex::new(p).unwrap().is_match(code))
        .map(|(_, what)| what.to_string())
        .collect();
    found.dedup();
    found
}

#[test]
fn only_the_pipeline_writes_graphics_escapes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    files.sort();
    let mut test_files = HashSet::new();
    let code: Vec<(PathBuf, String)> = files
        .iter()
        .map(|f| {
            let text = fs::read_to_string(f).unwrap();
            (f.clone(), without_tests(f, &text, &mut test_files))
        })
        .collect();
    let allowed: Vec<&str> = ALLOWED.iter().map(|(f, _)| *f).collect();
    let offending: Vec<String> = code
        .iter()
        .filter(|(f, _)| !test_files.contains(f))
        .filter_map(|(f, code)| {
            let name = f.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            let found = escapes(code);
            (!allowed.contains(&name.as_str()) && !found.is_empty()).then(|| {
                format!(
                    "{name} writes {}; describe the picture (src/ui/images.rs) or give it \
                     through src/mux/graphics.rs instead",
                    found.join(", ")
                )
            })
        })
        .collect();
    assert!(offending.is_empty(), "{}", offending.join("\n"));
    // Each file allowed still does, so the list doesn't outlive its reason.
    for (file, why) in ALLOWED {
        let (_, code) = code
            .iter()
            .find(|(f, _)| f.strip_prefix(root).unwrap() == Path::new(file))
            .unwrap_or_else(|| panic!("{file} ({why}) is gone"));
        assert!(!escapes(code).is_empty(), "{file} ({why}) no longer does");
    }
}

#[test]
fn the_check_leaves_out_tests_and_sees_every_spelling_of_esc() {
    let mut test_files = HashSet::new();
    let file = Path::new("/src/a/b.rs");
    let text = "fn f() -> &'static str {\n    \"\\u{1b}_Gi=1\"\n}\n\n#[cfg(test)]\nmod tests;\n\n#[cfg(test)]\nmod more {\n    const X: &str = \"\\x1bPq\";\n}\n\nfn g() {}\n";
    let code = without_tests(file, text, &mut test_files);
    assert!(code.contains("fn g()"), "{code}");
    assert!(!code.contains("\\x1bPq"), "{code}");
    assert_eq!(escapes(&code), vec!["ESC _ G".to_string()]);
    for spelled in [
        "\"\\x1B_G\"",
        "\"\\u{001b}_G\"",
        "\"\\u{01B}P0;1q\"",
        "[0x1b, b'_', b'G']",
        "[0x1B, b'P']",
    ] {
        assert!(!escapes(spelled).is_empty(), "{spelled}");
    }
    assert!(
        escapes("\"\\x1b[2J\\x1b_X\"").is_empty(),
        "other escapes pass"
    );
    assert!(test_files.contains(Path::new("/src/a/b/tests.rs")));
}

/// A server with UX 2.0 on and pictures drawn as sixel, and a terminal
/// attached (not Ghostty or kitty) that answered its cell is 13 by 25
/// pixels.
fn sixel_server(cols: u16, rows: u16) -> (TestServer, common::TestClient) {
    let server = TestServer::start_with_settings(
        r#"{"ux2": true, "image_protocol": "sixel", "notifications": false, "tips": false, "sidebar": false}"#,
    );
    let mut client = server.attach(cols, rows);
    client.wait_for("the header", |s| s[2].contains("M-c Claude"));
    client.wait_sent("the terminal asked for its cell", b"\x1b[16t");
    client.send(b"\x1b[6;25;13t");
    (server, client)
}

/// Whether a client was sent anything of kitty's: a graphics command, or a
/// placeholder cell.
fn kitty_sent(raw: &[u8]) -> bool {
    common::sent(raw, b"\x1b_G") || common::sent(raw, "\u{10EEEE}".as_bytes())
}

#[test]
fn with_sixel_the_settings_switches_reach_the_terminal_as_sixel() {
    let (server, mut client) = sixel_server(140, 40);
    server.ok(&["press", "settings"]);
    // A switch is four cells by one: 52 by 25 pixels, made 24, whole bands.
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    loop {
        client.screen();
        if common::sixels(&client.raw)
            .iter()
            .any(|(_, size)| *size == (52, 24))
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no switch drawn as sixel: {:?}\n{}",
            common::sixels(&client.raw),
            client.screen().join("\n")
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!kitty_sent(&client.raw), "nothing of kitty's sent");
}

#[test]
fn with_sixel_a_picture_shown_full_size_is_drawn_as_sixel_over_its_pane() {
    let (server, mut client) = sixel_server(100, 30);
    let file = server.dir.path().join("shot.png");
    common::four_colours(&file, 64);
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["picture", "-t", &pane, file.to_str().unwrap()]);
    client.wait_text("shot.png  Esc back");
    // The pane's width of the picture, at the 13 pixels a cell the terminal
    // answered, drawn off the server's task a moment after it shows.
    let deadline = common::deadline(std::time::Duration::from_secs(20));
    let (at, size) = loop {
        client.screen();
        if let Some(found) = common::sixels(&client.raw)
            .into_iter()
            .find(|(_, (w, _))| *w == 100 * 13)
        {
            break found;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no picture drawn as sixel: {:?}",
            common::sixels(&client.raw)
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(size.1 % 6, 0, "whole bands: {size:?}");
    assert!(size.1 > 20 * 25, "most of the pane's rows: {size:?}");
    let sixel = common::sixel_at(&client.raw, at.0, at.1).unwrap();
    for (color, rgb) in [
        ("red", ";2;100;0;0"),
        ("green", ";2;0;100;0"),
        ("blue", ";2;0;0;100"),
    ] {
        assert!(sixel.contains(rgb), "its {color} quarter");
    }
    assert!(!kitty_sent(&client.raw), "nothing of kitty's sent");
    // Gone with Esc: its cells erased.
    let before = client.raw.len();
    client.type_text("\x1b");
    client.wait_for("the picture gone", |s| {
        !s.iter().any(|l| l.contains("Esc back"))
    });
    let erased = format!("\x1b[{};{}H\x1b[100X", at.0 + 1, at.1 + 1);
    client.wait_sent("the picture erased", erased.as_bytes());
    assert!(common::sixels(&client.raw[before..]).is_empty());
}

#[test]
fn with_sixel_a_box_s_shadow_is_its_text_form() {
    let (server, mut client) = sixel_server(100, 30);
    server.ok(&["press", "palette"]);
    let screen = client.wait_for("the palette and its shadow", |s| {
        s.iter().any(|l| l.contains('╭')) && s.iter().any(|l| l.contains('▘'))
    });
    assert!(
        screen.iter().any(|l| l.contains('▀')),
        "half blocks under it:\n{}",
        screen.join("\n")
    );
    assert!(
        !common::sent(&client.raw, b"z=-1"),
        "no shadow under the text"
    );
    assert!(!kitty_sent(&client.raw));
}

#[test]
fn with_sixel_chosen_a_kitty_terminal_still_gets_kitty_s_way() {
    let server = TestServer::start_with_settings(
        r#"{"image_protocol": "sixel", "notifications": false, "tips": false}"#,
    );
    let mut ghostty = server.attach_as("xterm-ghostty", 100, 30);
    ghostty.wait_text("$");
    // The server's images by kitty's protocol: the shadow under the text.
    server.ok(&["press", "palette"]);
    ghostty.wait_sent("the shadow placed under the text", b"z=-1,C=1");
    ghostty.type_text("\x1b");
    // A pane's picture passed on as it came, its placeholders kept.
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["set", "-t", &pane, "@allow_passthrough", "1"]);
    server.show_picture(&mut ghostty, &pane);
    ghostty.wait_sent("the picture passed on", b"\x1b_Ga=T,U=1,f=100,t=f");
    assert!(common::sent(&ghostty.raw, "\u{10EEEE}".as_bytes()));
    assert!(common::sixels(&ghostty.raw).is_empty(), "no sixel");
    assert!(
        !common::sent(&ghostty.raw, b"\x1b[16t"),
        "not asked for a sixel cell"
    );
}

#[test]
fn with_sixel_a_picture_a_view_gives_inline_is_drawn_as_sixel_over_its_cells() {
    use switchboard::notes::images::Picture;
    let (server, mut client) = sixel_server(100, 30);
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["set", "-t", &pane, "@allow_passthrough", "1"]);
    // The four-colour picture given as a view's `ImageCache` gives one:
    // its data inline, in parts carrying only `m` after the first, then
    // its placeholder cells, four by two, each row after "Q8".
    let png = server.dir.path().join("four.png");
    common::four_colours(&png, 40);
    let data = switchboard::util::base64(&std::fs::read(&png).unwrap());
    let parts: Vec<&str> = data
        .as_bytes()
        .chunks(data.len().div_ceil(3))
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        let more = u8::from(i + 1 < parts.len());
        let keys = if i == 0 {
            format!("a=T,U=1,f=100,t=d,q=2,i=43,c=4,r=2,m={more}")
        } else {
            format!("m={more}")
        };
        out.push_str(&format!("\x1b_G{keys};{part}\x1b\\"));
    }
    // The pane cleared, the rows put at its first two, moved to.
    out.push_str("\x1b[H\x1b[2J");
    for row in 0..2u16 {
        out.push_str(&format!("\x1b[{};1HQ8\x1b[38;2;0;0;43m", row + 1));
        for col in 0..4 {
            out.push_str(&Picture::cell(row, col));
        }
        out.push_str("\x1b[0m");
    }
    out.push_str("\x1b[10;1H");
    let file = server.dir.path().join("inline.out");
    std::fs::write(&file, out).unwrap();
    server.ok(&[
        "send-keys",
        "-t",
        &pane,
        "-l",
        &format!("cat {}; echo inl''ine-done", file.display()),
    ]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    let screen = client.wait_text("inline-done");
    assert!(screen[4].starts_with("Q8"), "under the header: {screen:?}");
    // Four cells by two of 13 by 25 pixels, made whole bands, from the
    // pane's top left under UX 2.0's header.
    client.wait_sent("the picture drawn", b"\x1b[5;3H\x1bP0;1;0q\"1;1;52;48");
    let sixel = common::sixel_at(&client.raw, 4, 2).unwrap();
    for rgb in [";2;100;0;0", ";2;0;100;0", ";2;0;0;100", ";2;100;100;0"] {
        assert!(sixel.contains(rgb), "{rgb} in {sixel:?}");
    }
    assert!(!kitty_sent(&client.raw), "nothing of kitty's passed on");
}

/// Whether a client was sent either of the rounded ends' half circles as
/// glyphs.
fn half_circles_sent(raw: &[u8]) -> bool {
    common::sent(raw, "\u{E0B6}".as_bytes()) || common::sent(raw, "\u{E0B4}".as_bytes())
}

/// The one-cell sixels a client was sent on screen line `line`, for a cell
/// of 13 by 25 pixels (30, its height rounded up to whole bands).
fn ends_on(raw: &[u8], line: usize) -> usize {
    common::sixels(raw)
        .iter()
        .filter(|((l, _), size)| *l == line && *size == (13, 30))
        .count()
}

#[test]
fn with_sixel_the_pills_rounded_ends_are_one_cell_sixels_and_ghostty_keeps_its_glyphs() {
    let (server, mut client) = sixel_server(140, 30);
    // UX 2.0's header, its keycaps rounded at both ends, on line 2.
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while ends_on(&client.raw, 2) < 4 {
        client.screen();
        assert!(
            std::time::Instant::now() < deadline,
            "no rounded ends drawn as sixel on the header: {:?}",
            common::sixels(&client.raw)
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!half_circles_sent(&client.raw), "no half circle as a glyph");
    // Ghostty draws the glyphs itself, whatever is chosen.
    let mut ghostty = server.attach_as("xterm-ghostty", 140, 30);
    ghostty.wait_for("the header", |s| s[2].contains("Claude"));
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while !half_circles_sent(&ghostty.raw) {
        ghostty.screen();
        assert!(
            std::time::Instant::now() < deadline,
            "no glyphs for Ghostty"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(ends_on(&ghostty.raw, 2), 0, "and no sixel");
}

#[test]
fn by_default_a_terminal_that_answers_sixel_gets_it_and_windows_terminal_its_colors() {
    // No image protocol chosen: by the terminal.
    let server = TestServer::start_with_settings(
        r#"{"ux2": true, "notifications": false, "tips": false, "sidebar": false}"#,
    );
    let mut wt = server.attach_as("xterm-256color WindowsTerminal", 140, 30);
    wt.wait_for("the header", |s| s[2].contains("M-c Claude"));
    // Asked what it is (DA1); until it says, no sixel and no cell asked.
    wt.wait_sent("asked what it is", b"\x1b[c");
    assert!(!common::sent(&wt.raw, b"\x1b[16t"));
    // Windows Terminal 1.22 says it draws sixel (attribute 4).
    wt.send(b"\x1b[?61;4;6;7;14;21;22;23;24;28;32;42c");
    wt.wait_sent("asked for its sixel cell", b"\x1b[16t");
    wt.send(b"\x1b[6;25;13t");
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while ends_on(&wt.raw, 2) < 4 {
        wt.screen();
        assert!(
            std::time::Instant::now() < deadline,
            "no rounded ends as sixel for Windows Terminal: {:?}",
            common::sixels(&wt.raw)
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // The canvas's own colors, in truecolor.
    assert!(common::sent(&wt.raw, b"38;2;"), "truecolor");
    assert!(!kitty_sent(&wt.raw) && !half_circles_sent(&wt.raw));
    // One older, whose answer lists no sixel: the text forms, its colors
    // still the canvas's.
    let mut old = server.attach_as("xterm-256color WindowsTerminal", 140, 30);
    old.wait_for("the header", |s| s[2].contains("M-c Claude"));
    old.wait_sent("asked what it is", b"\x1b[c");
    old.send(b"\x1b[?61;6;7;22;23;24;28;32;42c");
    old.type_text("\x1b ");
    old.wait_text("MISSION CONTROL");
    assert!(!common::sent(&old.raw, b"\x1b[16t"));
    assert!(common::sixels(&old.raw).is_empty());
    assert!(!kitty_sent(&old.raw) && !half_circles_sent(&old.raw));
    assert!(common::sent(&old.raw, b"38;2;"), "truecolor");
    // A terminal that says nothing of itself: the 256 colors too.
    let mut plain = server.attach_as("xterm-256color", 140, 30);
    plain.wait_for("the header", |s| s[2].contains("M-c Claude"));
    assert!(common::sixels(&plain.raw).is_empty());
    assert!(!half_circles_sent(&plain.raw) && !common::sent(&plain.raw, b"38;2;"));
}

/// The files in src/ that may read a terminal by its name or the variables
/// that say what it is, and why: the one place that decides how anything
/// is drawn for a terminal, and those that say what a terminal is, pass
/// its variables on, or set one up.
const NAMES_ALLOWED: &[(&str, &str)] = &[
    (
        "src/mux/graphics.rs",
        "decides how anything is drawn for a terminal",
    ),
    (
        "src/mux/client.rs",
        "says what its terminal is as it attaches",
    ),
    ("src/ui/headless.rs", "attaches as Ghostty and says so"),
    (
        "src/mux/pane.rs",
        "tells the panes' programs they draw truecolor",
    ),
    (
        "src/demo/mod.rs",
        "keeps the terminal's variables for the demo",
    ),
    (
        "src/cmd/try_sealed.rs",
        "passes the terminal's variables into the sandbox",
    ),
    (
        "src/setup/terminal.rs",
        "sets up Ghostty's own configuration",
    ),
    (
        "src/setup/leftovers.rs",
        "tidies Ghostty's own configuration",
    ),
];

/// What a file's code reads a terminal by: its name in a test of a string
/// (`contains("ghostty")`, `== "kitty"`), the words that say it draws
/// truecolor, or the variables a terminal sets to say what it is.
fn terminal_names(code: &str) -> Vec<String> {
    let patterns = [
        r#"(contains\(|==)\s*"(ghostty|kitty|truecolor|24bit)"#,
        r#""(truecolor|24bit)""#,
        r#""(TERM_PROGRAM|COLORTERM|WT_SESSION|LC_TERMINAL)""#,
    ];
    patterns
        .iter()
        .filter_map(|p| regex::Regex::new(p).unwrap().find(code))
        .map(|m| m.as_str().to_string())
        .collect()
}

#[test]
fn only_the_pipeline_decides_by_what_a_terminal_is() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    files.sort();
    let mut test_files = HashSet::new();
    let code: Vec<(String, String)> = files
        .iter()
        .map(|f| {
            let text = fs::read_to_string(f).unwrap();
            let name = f.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            (name, without_tests(f, &text, &mut test_files))
        })
        .collect();
    let allowed: Vec<&str> = NAMES_ALLOWED.iter().map(|(f, _)| *f).collect();
    let offending: Vec<String> = code
        .iter()
        .filter(|(name, _)| !test_files.contains(&root.join(name)))
        .filter(|(name, _)| !allowed.contains(&name.as_str()))
        .filter_map(|(name, code)| {
            let found = terminal_names(code);
            (!found.is_empty()).then(|| {
                format!(
                    "{name} reads a terminal by {}; ask src/mux/graphics.rs (`for_server`, \
                     `ends`, `truecolor`, `pointer_shapes`, ...) instead",
                    found.join(", ")
                )
            })
        })
        .collect();
    assert!(offending.is_empty(), "{}", offending.join("\n"));
    for (file, why) in NAMES_ALLOWED {
        let (_, code) = code
            .iter()
            .find(|(name, _)| name == file)
            .unwrap_or_else(|| panic!("{file} ({why}) is gone"));
        assert!(
            !terminal_names(code).is_empty(),
            "{file} ({why}) no longer does"
        );
    }
    // And the checks themselves are the pipeline's alone.
    assert!(terminal_names(r#"term.contains("ghostty")"#).len() == 1);
    assert!(terminal_names(r#"std::env::var("WT_SESSION")"#).len() == 1);
    assert!(terminal_names(r#"let name = "my ghostty notes";"#).is_empty());
}

/// Has `pane` print `text` with printf (its octal escapes, so the line
/// typed holds no ESC) and then `done` split by quotes, and waits for it.
fn printed(
    server: &TestServer,
    client: &mut common::TestClient,
    pane: &str,
    text: &str,
    done: &str,
) {
    let (head, tail) = done.split_at(done.len() / 2);
    server.ok(&[
        "send-keys",
        "-t",
        pane,
        "-l",
        &format!("printf '{text}'; echo {head}''{tail}"),
    ]);
    server.ok(&["send-keys", "-t", pane, "Enter"]);
    client.wait_text(done);
}

/// A half circle (U+E0B6, as printf's octal escapes) in a color, then
/// `word`.
fn half_circle_then(word: &str) -> String {
    format!("\\033[38;2;0;128;255m\\356\\202\\266\\033[0m{word}\\n")
}

fn first_pane_of(server: &TestServer) -> String {
    server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn with_sixel_a_program_s_half_circles_are_its_own_and_switchboard_s_apps_become_pictures() {
    let (server, mut client) = sixel_server(100, 30);
    let pane = first_pane_of(&server);
    // A program of yours (a powerline prompt): the glyph, left to the
    // terminal.
    printed(
        &server,
        &mut client,
        &pane,
        &half_circle_then("mine"),
        "mine-done",
    );
    assert!(half_circles_sent(&client.raw), "the program's own glyph");
    // In a pane Switchboard's app draws in (allowed pictures): a picture.
    server.ok(&["set", "-t", &pane, "@allow_passthrough", "1"]);
    let before = client.raw.len();
    printed(
        &server,
        &mut client,
        &pane,
        &half_circle_then("ours"),
        "ours-done",
    );
    let screen = client.screen();
    let line = screen
        .iter()
        .position(|l| l.contains("ours") && !l.contains("printf"))
        .unwrap_or_else(|| panic!("the line: {}", screen.join("\n")));
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while ends_on(&client.raw[before..], line) == 0 {
        client.screen();
        assert!(
            std::time::Instant::now() < deadline,
            "the app's half circle not drawn as sixel: {:?}",
            common::sixels(&client.raw[before..])
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn the_apps_ask_what_the_session_s_terminals_get_and_one_that_gets_neither_shows_blanks() {
    // By the terminal, as by default.
    let server = TestServer::start_with_settings(
        r#"{"ux2": true, "notifications": false, "tips": false, "sidebar": false}"#,
    );
    let mut first = server.attach_as("xterm-256color", 100, 30);
    // Its session made before its pane is looked for.
    first.wait_for("the header", |s| s[2].contains("M-c Claude"));
    let pane = first_pane_of(&server);
    let graphics = || {
        server
            .ok(&["show", "-t", &pane, "@graphics"])
            .trim()
            .to_string()
    };
    first.detach();
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while !graphics().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "unknown with no terminal attached: {}",
            graphics()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let mut plain = server.attach_as("xterm-256color", 100, 30);
    plain.wait_for("the header", |s| s[2].contains("M-c Claude"));
    assert_eq!(graphics(), "none");
    // Windows Terminal, before it answers: it may get sixel, round.
    let mut wt = server.attach_as("xterm-256color WindowsTerminal", 100, 30);
    wt.wait_sent("asked what it is", b"\x1b[c");
    assert_eq!(graphics(), "sixel round");
    wt.send(b"\x1b[?61;4;6;7c");
    wt.wait_sent("asked for its sixel cell", b"\x1b[16t");
    wt.send(b"\x1b[6;25;13t");
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while graphics() != "sixel round" {
        assert!(std::time::Instant::now() < deadline, "{}", graphics());
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // Switchboard's app in the pane draws for Windows Terminal: a picture's
    // placeholders and a half circle. The terminal that gets neither has
    // them blank and a space of its color, never boxes or a glyph.
    server.ok(&["set", "-t", &pane, "@allow_passthrough", "1"]);
    let before = plain.raw.len();
    printed(
        &server,
        &mut plain,
        &pane,
        &half_circle_then("app"),
        "app-done",
    );
    server.show_picture(&mut plain, &pane);
    assert!(!common::sent(&plain.raw[before..], "\u{10EEEE}".as_bytes()));
    assert!(!half_circles_sent(&plain.raw[before..]));
    // A Ghostty: kitty's, with its glyphs.
    let mut ghostty = server.attach_as("xterm-ghostty", 100, 30);
    ghostty.wait_for("the header", |s| s[2].contains("Claude"));
    assert_eq!(graphics(), "kitty sixel round");
}
