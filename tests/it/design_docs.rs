//! The design-docs project skill's renderer (`.claude/skills/design-docs/scripts/render.js`),
//! which writes the UX 2.0 canvas's boards as docs/design's web pages: its self-test checks a
//! grid board's page has no line breaks between its tags while a Slack board's keeps its spaces,
//! and its made-up-data check, then draws both with Chrome, checking each picture's size, that it
//! isn't blank, and that the grid board's meter is on its text's row, so a one-row shift fails.
//! It needs Node, so it is skipped without it, and the drawing needs Chrome (the script exits 77
//! when it finds none, having checked the rest). Its fixtures load nothing from the network, and
//! Chrome runs with a profile of its own under the test's temporary home.

use crate::common;

use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use common::TempDir;

#[test]
fn the_renderer_keeps_a_grid_boards_text_on_its_meters_and_finds_names_not_made_up() {
    let home = TempDir::new("design-docs");
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".claude/skills/design-docs/scripts/render.js");
    let out = match Command::new("node")
        .arg(&script)
        .arg("--self-test")
        .env("HOME", home.path())
        .env("TMPDIR", home.path())
        .output()
    {
        Err(e) if e.kind() == ErrorKind::NotFound => return eprintln!("skipped: no node"),
        result => result.unwrap(),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let drew = out.status.code() != Some(77);
    assert!(
        out.status.success() || !drew,
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("ok grid page: no line breaks between its tags"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ok slack page: its spaces between tags kept"),
        "{stdout}"
    );
    if !drew {
        return eprintln!("{stdout}");
    }
    assert!(stdout.contains("ok grid.dc.html: 1280×800"), "{stdout}");
    assert!(
        stdout.contains("ok grid meter on its text's row"),
        "{stdout}"
    );
    assert!(stdout.contains("ok slack.dc.html: 420×880"), "{stdout}");
}

#[test]
fn the_renderers_chrome_asks_macos_for_no_keychain() {
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".claude/skills/design-docs/scripts/render.js");
    let out = match Command::new("node")
        .arg(&script)
        .arg("--print-chrome-args")
        .output()
    {
        Err(e) if e.kind() == ErrorKind::NotFound => return eprintln!("skipped: no node"),
        result => result.unwrap(),
    };
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let switches: Vec<&str> = stdout.lines().collect();
    for wanted in ["--use-mock-keychain", "--password-store=basic"] {
        assert!(switches.contains(&wanted), "{stdout}");
    }
}
