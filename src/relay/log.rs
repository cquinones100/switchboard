//! The relay's log (lib/slack-relay/util.js's `log` and `fail`): each line
//! to its tab, with the time, and to `relay.log`, which outlives the tab and
//! is how a crash gets explained (see `run::last_error`). Everything the
//! relay writes is its owner's own, so the log is readable by them alone,
//! whatever the umask.
//!
//! The same failure again within `REPEAT_MS` is counted rather than written,
//! so one that repeats every few seconds doesn't bury the rest; the next
//! time it is written, it says how many times it came in between.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::sync::LazyLock;

use regex::Regex;

use super::config::Paths;
use crate::util;

pub const REPEAT_MS: i64 = 10 * 60_000;

/// An exit with this code is a mistake in how the relay was started (with
/// another listener running, without its tokens), which a restart would
/// only repeat.
pub const DONT_RESTART: u8 = 3;

static FAILURE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)failed|error|reconnecting").expect("a regex"));

/// The log: whether lines are echoed to the tab too, and the failures seen
/// lately with how often they came since.
pub struct Log {
    pub echo: bool,
    repeats: RefCell<HashMap<String, (i64, u32)>>,
    order: RefCell<Vec<String>>,
}

impl Log {
    pub fn new(echo: bool) -> Log {
        Log {
            echo,
            repeats: RefCell::new(HashMap::new()),
            order: RefCell::new(Vec::new()),
        }
    }

    /// Writes a line, as the module's comment says.
    pub fn line(&self, paths: &Paths, line: &str) {
        let mut line = line.to_string();
        if FAILURE.is_match(&line) {
            let now = util::now_ms();
            let mut repeats = self.repeats.borrow_mut();
            let seen = repeats.get(&line).copied();
            if let Some((at, count)) = seen
                && now - at < REPEAT_MS
            {
                repeats.insert(line, (at, count + 1));
                return;
            }
            repeats.insert(line.clone(), (now, 0));
            let mut order = self.order.borrow_mut();
            order.retain(|l| *l != line);
            order.push(line.clone());
            if order.len() > 200 {
                let old = order.remove(0);
                repeats.remove(&old);
            }
            if let Some((_, count)) = seen
                && count > 0
            {
                line.push_str(&format!(
                    " (and {count} more time{} since the last time logged)",
                    if count == 1 { "" } else { "s" }
                ));
            }
        }
        if self.echo {
            let time = util::local_now().strftime("%-I:%M:%S %p").to_string();
            let mut out = std::io::stdout().lock();
            // The tab's terminal may be gone (closing it is a stop).
            let _ = writeln!(out, "{time} {line}");
        }
        append(paths, &format!("{} {line}\n", iso_now()));
    }
}

/// Now, as `new Date().toISOString()` writes it.
pub fn iso_now() -> String {
    iso(util::now_ms())
}

/// An epoch time in ms, as `toISOString()` writes it.
pub fn iso(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .strftime("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// Appends to the log, owner-only.
fn append(paths: &Paths, text: &str) {
    let _ = std::fs::create_dir_all(&paths.state_dir);
    let _ = util::write_private(paths.log_file(), text, true);
}

/// Says what went wrong, to stderr and the log, and gives the code to exit
/// with (util.js's `fail`).
pub fn fail(paths: &Paths, message: &str, code: u8) -> std::process::ExitCode {
    eprintln!("Error: {message}");
    append(paths, &format!("{} Error: {message}\n", iso_now()));
    std::process::ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_that_repeats_is_written_once_a_while() {
        let dir = std::env::temp_dir().join(format!("relay-log-{}", std::process::id()));
        let paths = Paths::under(&dir, &dir.join("s"));
        let log = Log::new(false);
        for _ in 0..3 {
            log.line(
                &paths,
                "dashboard failed: views.publish: not_enabled (log test)",
            );
        }
        log.line(&paths, "a line for the log");
        let text = std::fs::read_to_string(paths.log_file()).unwrap();
        assert_eq!(text.matches("(log test)").count(), 1);
        assert!(
            Regex::new(r"(?m)^\S+Z a line for the log$")
                .unwrap()
                .is_match(&text)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn times_read_as_javascript_writes_them() {
        assert_eq!(iso(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
    }
}
