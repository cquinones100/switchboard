//! `/walkthrough`'s terminal recorder (assets/skills/walkthrough/scripts/
//! record-terminal.js), through its `--self-test`, which checks what needs
//! no server: the timeline a walk of two steps and three keys makes, and,
//! where its Playwright packages and Chrome are there, a video made from
//! made-up frames with both strips. Skipped where Node or ffmpeg isn't on
//! the PATH, since the scripts have no test runner of their own.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::common::TempDir;

fn on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|file| file.is_file())
}

#[test]
fn the_terminal_recorder_times_its_frames_and_frames_its_video() {
    let (Some(node), Some(_)) = (on_path("node"), on_path("ffmpeg")) else {
        eprintln!("skipped: the recorder needs Node and ffmpeg on the PATH");
        return;
    };
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/skills/walkthrough/scripts/record-terminal.js");
    let dir = TempDir::new("sb-walkthrough");
    let out = Command::new(node)
        .arg(&script)
        .arg("--self-test")
        .current_dir(&dir.0)
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "the self-test printed no report: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(report["timeline"], "ok", "{report}");
    let video = report["video"].as_str().unwrap_or_default();
    assert!(video == "ok" || video.starts_with("skipped"), "{report}");
    assert!(out.status.success(), "{report}");
}
