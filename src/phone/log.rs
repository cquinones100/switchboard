//! The phone app's own log, `~/.local/state/switchboard/phone/phone.log`:
//! what `switchboard phone run` says (where it serves, who asked to pair,
//! what it refused) and each push's outcome, a line each with its time,
//! kept to `MOST` bytes by dropping its older half. What it says goes to
//! its stderr too, the tab it runs in. Before it, the run wrote only to
//! stderr, which went wherever whoever started it pointed it, so a push
//! that failed left nothing to read.

use std::path::{Path, PathBuf};

use super::config;
use crate::util;

/// How large the log grows before its older half goes.
const MOST: u64 = 256 * 1024;

pub fn file() -> PathBuf {
    file_in(&config::state_dir())
}

fn file_in(dir: &Path) -> PathBuf {
    dir.join("phone.log")
}

/// `text` said on stderr and kept in the log.
pub fn line(text: &str) {
    eprintln!("phone: {text}");
    line_in(&config::state_dir(), text);
}

/// `text` kept in the log in `dir`, with the time.
pub fn line_in(dir: &Path, text: &str) {
    let path = file_in(dir);
    let at = util::local_at_ms(util::now_ms()).strftime("%Y-%m-%d %H:%M:%S");
    util::append_quietly(&path, format!("{at} {}\n", text.replace('\n', " ")));
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MOST) {
        trim(&path);
    }
}

/// The log's newer half kept, from the first whole line in it.
fn trim(path: &Path) {
    let Some(text) = util::read_text(path) else {
        return;
    };
    let half = text.len() / 2;
    let from = text[half..]
        .find('\n')
        .map_or(text.len(), |at| half + at + 1);
    let _ = util::write_atomic(path, &text[from..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_keeps_its_newer_half_once_it_is_full() {
        let dir = std::env::temp_dir().join(format!("sb-phone-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let long = "x".repeat(1000);
        for n in 0..300 {
            line_in(&dir, &format!("{n} {long}"));
        }
        let kept = std::fs::read_to_string(file_in(&dir)).unwrap();
        assert!(kept.len() as u64 <= MOST, "{}", kept.len());
        assert!(kept.lines().last().unwrap().contains(" 299 "));
        assert!(!kept.contains(" 0 x"), "the oldest lines went");
        assert!(
            kept.lines().all(|l| l.len() > 1000),
            "only whole lines are kept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
