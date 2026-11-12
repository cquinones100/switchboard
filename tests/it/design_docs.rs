//! The design-docs project skill's renderer (`.claude/skills/design-docs/scripts/render.js`),
//! which draws the UX 2.0 canvas's boards as docs/design's PNGs: its self-test draws a grid
//! board and a Slack board kept beside it and checks each picture's size and that it isn't
//! blank, and its made-up-data check. It needs Node and Chrome, so it is skipped where either is
//! missing (the script exits 77 when it finds no Chrome). Its fixtures load nothing from the
//! network, and Chrome runs with a profile of its own under the test's temporary home.

use crate::common;

use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use common::TempDir;

#[test]
fn the_renderer_draws_its_fixtures_at_their_size_and_finds_names_not_made_up() {
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
    if out.status.code() == Some(77) {
        return eprintln!("{stdout}");
    }
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("ok grid.dc.html: 1280×800"), "{stdout}");
    assert!(stdout.contains("ok slack.dc.html: 420×880"), "{stdout}");
}
