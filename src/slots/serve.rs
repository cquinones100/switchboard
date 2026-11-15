//! The queue for heavy work on the server: `switchboard slot-report`, which
//! the stand-ins in the panes send as a command starts waiting, takes its
//! slot and lets it go, kept in `Server::slots`, and the tick's check that
//! each reporting process still runs, so one killed before it said so
//! leaves no tab marked waiting. The sidebar, the cards and the Resources
//! view read the board; nothing here reads a lock file.

use super::{Hold, Kind, Use};
use crate::mux::PaneId;
use crate::mux::server::{Reply, Server};
use crate::util;

/// What `slot-report` takes: `wait <kind> <pid> -- <command>`, `hold <kind>
/// <pid> <index> <cap> [child] -- <command>`, or `done <pid>`.
pub fn parse(rest: &[String], pane: PaneId, now_ms: i64) -> Result<Report, String> {
    let words: Vec<&str> = rest.iter().map(String::as_str).collect();
    let split = words.iter().position(|w| *w == "--").unwrap_or(words.len());
    let (head, command) = (
        &words[..split],
        words.get(split + 1..).unwrap_or(&[]).join(" "),
    );
    let number = |w: Option<&&str>| w.and_then(|w| w.parse::<usize>().ok());
    // A pid of 0 would name the server's own process group to a stop.
    let pid_of = |w: Option<&&str>| w.and_then(|w| w.parse::<u32>().ok()).filter(|p| *p > 0);
    let pid = pid_of;
    let bad = || {
        "slot-report wait|hold <kind> <pid> [index cap child] -- command, or done <pid>".to_string()
    };
    match head.first().copied() {
        Some("done") => pid(head.get(1)).map(Report::Done).ok_or_else(bad),
        Some(how @ ("wait" | "hold")) => {
            let kind = head
                .get(1)
                .and_then(|k| Kind::from_name(k))
                .ok_or_else(bad)?;
            let pid = pid(head.get(2)).ok_or_else(bad)?;
            let hold = if how == "wait" {
                Hold::Waiting
            } else {
                Hold::Holding {
                    index: number(head.get(3)).ok_or_else(bad)?,
                    cap: number(head.get(4)).ok_or_else(bad)?,
                }
            };
            let child = if how == "hold" {
                pid_of(head.get(5))
            } else {
                None
            };
            Ok(Report::Use(Use {
                pid,
                child,
                pane,
                kind,
                hold,
                since: now_ms.div_euclid(1000),
                came: now_ms,
                command,
            }))
        }
        _ => Err(bad()),
    }
}

/// A report read.
#[derive(Debug, PartialEq)]
pub enum Report {
    Use(Use),
    Done(u32),
}

/// Whether a process runs, by a signal of none.
fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 sends nothing; it only asks whether the process is
    // there.
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

impl Server {
    /// `slot-report`, from a stand-in in the calling pane.
    pub fn cmd_slot_report(&mut self, rest: &[String], caller: Option<PaneId>) -> Reply {
        let Some(pane) = caller.filter(|p| self.panes.contains_key(p)) else {
            return Reply::ok("");
        };
        match parse(rest, pane, util::now_ms()) {
            Ok(Report::Use(mut u)) => {
                // A wait said again keeps its place in line.
                if let Some(before) = self.slots.get(u.pid)
                    && before.hold == Hold::Waiting
                    && u.hold == Hold::Waiting
                {
                    u.came = before.came;
                    u.since = before.since;
                }
                self.slots.report(u);
                self.dirty_slots();
                Reply::ok("")
            }
            Ok(Report::Done(pid)) => {
                if self.slots.end(pid) {
                    self.dirty_slots();
                }
                Reply::ok("")
            }
            Err(e) => Reply::fail(e),
        }
    }

    /// The tick's share: drops a report whose process has gone.
    pub fn slots_tick(&mut self) {
        if !self.slots.is_empty() && self.slots.retain(alive) {
            self.dirty_slots();
        }
    }

    /// Every client drawn again: the waiting tab's row, the cards and the
    /// Resources view all show the slots.
    fn dirty_slots(&mut self) {
        self.usage_slots_changed();
        for c in self.clients.values_mut() {
            c.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn reports_read_as_a_wait_a_hold_or_an_end() {
        let pane = PaneId(7);
        let Report::Use(wait) = parse(
            &words("wait build 42 -- cargo build --release"),
            pane,
            5_500,
        )
        .unwrap() else {
            panic!("a wait");
        };
        assert_eq!(
            (
                wait.kind,
                wait.hold,
                wait.pid,
                wait.since,
                wait.command.as_str()
            ),
            (Kind::Build, Hold::Waiting, 42, 5, "cargo build --release")
        );
        let Report::Use(hold) =
            parse(&words("hold tests 42 1 2 43 -- cargo test"), pane, 0).unwrap()
        else {
            panic!("a hold");
        };
        assert_eq!(hold.hold, Hold::Holding { index: 1, cap: 2 });
        assert_eq!((wait.child, hold.child), (None, Some(43)));
        assert_eq!(parse(&words("done 42"), pane, 0), Ok(Report::Done(42)));
        assert!(parse(&words("hold build 42"), pane, 0).is_err());
        assert!(parse(&words("wait laundry 1"), pane, 0).is_err());
        assert!(parse(&words("wait build 0 -- cargo build"), pane, 0).is_err());
    }

    #[test]
    fn a_process_that_ended_is_not_alive() {
        assert!(alive(std::process::id()));
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!alive(pid));
    }
}
