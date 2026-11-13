//! M-p's "Restart Switchboard on the latest build on disk" as the workspace
//! shows it while it goes (UX 2.0 board 56, "A build running without
//! blocking"): the sidebar's build row, the same words on the status line
//! with UX 2.0 off, and its steps in a box a click on the row (or a second
//! press) opens beside it. Nothing here blocks: the row is drawn from what
//! the server keeps of the build (`Build`, told by src/mux/latest.rs's
//! `Report`s), and the steps box takes no keys, so what you type still
//! reaches the tab while it is up; a click anywhere closes it, and it
//! closes itself once the build ends. After the restart the new server
//! shows "Restarted on <hash>" for `RESTARTED_FOR`; a failure stays in red
//! until its row is clicked, which shows why.
//!
//! New with the Rust port; the tmux version said only "Building…" once.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::style::Style;
use ratatui::text::Span;

use super::input::{Key, Mouse};
use super::latest::Report;
use crate::paths;
use crate::sidebar::text::SPINNER;
use crate::ui::Theme;
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Place};
use crate::ui::kit::{self, Col, Tone};
use crate::ui::{Look, Outcome, Overlay, Step};

/// How long the row says the restart is done.
pub const RESTARTED_FOR: Duration = Duration::from_secs(10);

/// How the main clone's update went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetch {
    /// Not a main clone: built as it is.
    None,
    Going,
    /// How many commits it brought (`None`: origin wasn't fetched), and
    /// how long it took.
    Done {
        commits: Option<usize>,
        took: Duration,
    },
}

/// What the server keeps of M-p's build under way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Build {
    pub started: Instant,
    pub fetch: Fetch,
    /// "main at 1a2b3c4", once known.
    pub head: String,
    /// When cargo started, and how long it took once it is done.
    pub building: Option<Instant>,
    pub built_in: Option<Duration>,
    /// Crates built, and how many the last build counted.
    pub done: usize,
    pub of: Option<usize>,
    /// Where the build is going in place of the server's binary.
    pub install: Option<PathBuf>,
    pub restarting: bool,
}

impl Build {
    pub fn new(main_clone: bool, now: Instant) -> Build {
        Build {
            started: now,
            fetch: if main_clone {
                Fetch::Going
            } else {
                Fetch::None
            },
            head: String::new(),
            building: None,
            built_in: None,
            done: 0,
            of: None,
            install: None,
            restarting: false,
        }
    }

    /// Takes what the build said.
    pub fn take(&mut self, report: Report, now: Instant) {
        match report {
            Report::Fetched { commits, took } => self.fetch = Fetch::Done { commits, took },
            Report::Building { head, of, .. } => {
                self.head = head;
                self.of = of;
                self.building = Some(now);
            }
            Report::Built(n) => self.done = n,
            Report::Installing(to) => {
                self.built_in = self.building.map(|at| now.duration_since(at));
                self.install = Some(to);
            }
            Report::Failed(_) => {}
        }
    }

    /// The step it is on.
    pub fn stage(&self) -> Stage {
        if self.restarting {
            Stage::Restart
        } else if self.install.is_some() {
            Stage::Install
        } else if self.building.is_some() || self.fetch == Fetch::None {
            Stage::Build
        } else {
            Stage::Fetch
        }
    }

    /// The meter's crates built over the last count, until the build is
    /// done; none the first time.
    fn meter(&self) -> Option<(usize, usize)> {
        let of = self.of.filter(|n| *n > 0)?;
        (self.stage() == Stage::Build).then_some((self.done.min(of), of))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Fetch,
    Build,
    Install,
    Restart,
}

/// What the row shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    Building(Build),
    /// Why, cargo's error lines when it was cargo that failed, and the
    /// build as it stood.
    Failed {
        build: Build,
        why: String,
        lines: Vec<String>,
    },
    Restarted {
        hash: String,
        until: Instant,
    },
}

impl Shown {
    /// The row, as of `now`; `None` once a restart's line has had its time.
    pub fn row(&self, now: Instant) -> Option<Row> {
        Some(match self {
            Shown::Building(b) => match b.stage() {
                Stage::Restart => Row::Working {
                    what: "Restarting",
                    meter: None,
                    secs: None,
                    note: "every pane kept",
                },
                stage => Row::Working {
                    what: match stage {
                        Stage::Fetch => "Updating",
                        Stage::Install => "Installing",
                        _ => "Building",
                    },
                    meter: b.meter(),
                    secs: Some(now.duration_since(b.started).as_secs()),
                    note: "",
                },
            },
            Shown::Failed { lines, .. } => Row::Failed {
                cargo: !lines.is_empty(),
            },
            Shown::Restarted { until, .. } if now >= *until => return None,
            Shown::Restarted { hash, .. } => Row::Restarted(hash.clone()),
        })
    }
}

/// The build row as it is drawn: a plain function of what is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Working {
        what: &'static str,
        meter: Option<(usize, usize)>,
        secs: Option<u64>,
        note: &'static str,
    },
    Restarted(String),
    /// `cargo`: cargo failed, rather than the update before it.
    Failed {
        cargo: bool,
    },
}

/// Seconds as the row says them: "41s", "2m 05s".
pub fn took(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

/// The meter's cells in the sidebar's row and the steps box.
const CELLS: usize = 10;

fn percent((done, of): (usize, usize)) -> u8 {
    (done * 100 / of.max(1)).min(100) as u8
}

impl Row {
    /// Whether its mark turns, so whatever draws it keeps the spinner going.
    pub fn spins(&self) -> bool {
        matches!(self, Row::Working { .. })
    }

    fn failed_words(cargo: bool) -> &'static str {
        if cargo {
            "Build failed · click for why"
        } else {
            "Not restarted · click for why"
        }
    }

    /// The row's spans, `cols` wide with the sidebar's space before them:
    /// the mark, the words, the meter and the time at the right edge, laid
    /// out by `kit::columns`, the meter and then the time dropped first
    /// when narrow.
    pub fn spans(&self, cols: usize, frame: usize, theme: &Theme) -> Vec<Span<'static>> {
        let mut line: Vec<Span<'static>> = vec![Span::raw(" ")];
        let inner = cols.saturating_sub(2);
        match self {
            Row::Working {
                what,
                meter,
                secs,
                note,
            } => {
                let spin = Span::styled(SPINNER[frame % SPINNER.len()], Tone::Working.bold(theme));
                let time = secs.map(took).unwrap_or_default();
                let words = if note.is_empty() {
                    what.to_string()
                } else {
                    format!("{what}  {note}")
                };
                let spec = [
                    Col::Fixed {
                        min: 1,
                        natural: 1,
                        cap: 1,
                    },
                    Col::Fixed {
                        min: 4,
                        natural: draw::width(&words),
                        cap: draw::width(&words),
                    },
                    Col::Stretch {
                        weight: 1,
                        min: if meter.is_some() { 4 } else { 0 },
                        max: Some(CELLS),
                    },
                    Col::Right {
                        natural: draw::width(&time),
                        cap: draw::width(&time),
                    },
                ];
                let at = kit::columns(&spec, inner, 1, &[2, 3]);
                let mut row: Vec<Span<'static>> = Vec::new();
                if let Some(a) = at[0] {
                    kit::place(&mut row, vec![spin], a);
                }
                if let Some(a) = at[1] {
                    let mut spans =
                        vec![Span::styled(what.to_string(), Tone::Working.style(theme))];
                    if !note.is_empty() {
                        spans.push(Span::styled(format!("  {note}"), Tone::Dim.style(theme)));
                    }
                    kit::place(&mut row, spans, a);
                }
                if let (Some(a), Some(m)) = (at[2], meter) {
                    kit::place(
                        &mut row,
                        kit::meter(percent(*m), a.1, Tone::Working, theme),
                        a,
                    );
                }
                if let Some(a) = at[3].filter(|_| !time.is_empty()) {
                    kit::place(
                        &mut row,
                        vec![Span::styled(time, Tone::Dim.style(theme))],
                        a,
                    );
                }
                line.extend(row);
            }
            Row::Restarted(hash) => {
                line.push(Span::styled("✓", Tone::Done.bold(theme)));
                line.push(Span::styled(
                    format!(" Restarted on {hash}"),
                    Tone::Done.style(theme),
                ));
            }
            Row::Failed { cargo } => {
                line.push(Span::styled("✕", Tone::Failed.bold(theme)));
                line.push(Span::styled(
                    format!(" {}", Row::failed_words(*cargo)),
                    Tone::Failed.style(theme),
                ));
            }
        }
        kit::fit(line, cols, Style::default())
    }

    /// The same in words, for the status line with UX 2.0 off.
    pub fn words(&self, frame: usize) -> String {
        match self {
            Row::Working {
                what,
                meter,
                secs,
                note,
            } => {
                let mut out = format!("{} {what}", SPINNER[frame % SPINNER.len()]);
                if let Some((done, of)) = meter {
                    out.push_str(&format!(" {done}/~{of}"));
                }
                if !note.is_empty() {
                    out.push_str(&format!(", {note}"));
                }
                if let Some(s) = secs {
                    out.push_str(&format!(" · {}", took(*s)));
                }
                out
            }
            Row::Restarted(hash) => format!("✓ Restarted on {hash}"),
            Row::Failed { cargo } => format!("✕ {}", Row::failed_words(*cargo)),
        }
    }
}

/// The steps box's lines: where the build is, each step's mark, words and
/// what it has to say, the meter while it builds, and on a failure the
/// failing step's error.
pub fn steps_lines(shown: &Shown, now: Instant, theme: &Theme) -> Vec<BoxLine> {
    let (build, failed) = match shown {
        Shown::Building(b) => (b, None),
        Shown::Failed { build, why, lines } => (build, Some((why, lines))),
        Shown::Restarted { hash, .. } => {
            return vec![BoxLine::spans(vec![
                Span::styled(" ✓", Tone::Done.bold(theme)),
                Span::styled(format!(" Restarted on {hash}"), Tone::Done.style(theme)),
            ])];
        }
    };
    let frame = (now.duration_since(build.started).as_millis() / 120) as usize;
    let stage = build.stage();
    let mut out = Vec::new();
    if !build.head.is_empty() {
        out.push(BoxLine::text(
            format!(" {}", build.head),
            Tone::Dim.style(theme),
        ));
        out.push(BoxLine::blank());
    }
    let step = |out: &mut Vec<BoxLine>, this: Stage, words: String, note: String| {
        let (mark, tone) = if this < stage {
            ("✓".to_string(), Tone::Done)
        } else if this > stage {
            ("○".to_string(), Tone::Dim)
        } else if failed.is_some() {
            ("✕".to_string(), Tone::Failed)
        } else {
            (SPINNER[frame % SPINNER.len()].to_string(), Tone::Working)
        };
        let words_style = if this > stage {
            Tone::Dim.style(theme)
        } else {
            Style::new().fg(theme.text)
        };
        let mut spans = vec![
            Span::styled(format!(" {mark} "), tone.bold(theme)),
            Span::styled(words, words_style),
        ];
        if !note.is_empty() {
            spans.push(Span::styled(format!("  ({note})"), Tone::Dim.style(theme)));
        }
        out.push(BoxLine::spans(spans));
    };
    let secs_since = |at: Instant| took(now.duration_since(at).as_secs());
    match &build.fetch {
        Fetch::None => {}
        Fetch::Going => step(
            &mut out,
            Stage::Fetch,
            "Fetching".into(),
            secs_since(build.started),
        ),
        Fetch::Done { commits, took: t } => {
            let note = match commits {
                Some(1) => format!("1 commit, {}", took(t.as_secs())),
                Some(n) => format!("{n} commits, {}", took(t.as_secs())),
                None => "origin not fetched".to_string(),
            };
            step(&mut out, Stage::Fetch, "Fetched".into(), note);
        }
    }
    let of = build.of.map(|n| format!("~{n}"));
    let build_note = match (stage, build.building) {
        (Stage::Build, Some(at)) => match &of {
            Some(of) => format!("{} of {of}, {}", build.done, secs_since(at)),
            None => format!("{}, {}", build.done, secs_since(at)),
        },
        (Stage::Install | Stage::Restart, _) => match build.built_in {
            Some(t) => format!("{} crates, {}", build.done, took(t.as_secs())),
            None => format!("{} crates", build.done),
        },
        _ => String::new(),
    };
    let build_words = if stage > Stage::Build {
        "Built"
    } else {
        "Building"
    };
    step(&mut out, Stage::Build, build_words.into(), build_note);
    let to = build
        .install
        .as_ref()
        .and_then(|p| p.parent())
        .map(paths::tilde)
        .unwrap_or_else(|| "~/.local/bin".to_string());
    step(&mut out, Stage::Install, "Install".into(), to);
    step(
        &mut out,
        Stage::Restart,
        "Restart".into(),
        "panes kept".into(),
    );
    if let Some(m) = build.meter().filter(|_| failed.is_none()) {
        out.push(BoxLine::blank());
        let mut spans = vec![Span::raw(" ")];
        spans.extend(kit::meter(percent(m), CELLS * 2, Tone::Working, theme));
        out.push(BoxLine::spans(spans));
    }
    if let Some((why, lines)) = failed {
        out.push(BoxLine::blank());
        let said: Vec<String> = if lines.is_empty() {
            draw::wrap(why, 60)
        } else {
            lines.clone()
        };
        for l in said {
            out.push(BoxLine::text(format!(" {l}"), Tone::Failed.style(theme)));
        }
    }
    out
}

/// What the steps box says at its foot.
const FOOT: &str = "Keys still reach the tab; a click closes this.";

/// The build's steps in a box beside the sidebar's build row, which takes
/// no keys and dims nothing.
pub struct Steps {
    pub shown: Shown,
    /// Its top-left corner on the screen.
    at: (u16, u16),
    drawn: Option<Drawn>,
}

impl Steps {
    pub fn new(shown: Shown, at: (u16, u16)) -> Steps {
        Steps {
            shown,
            at,
            drawn: None,
        }
    }
}

impl Overlay for Steps {
    fn drawn(&self) -> Option<&crate::ui::draw::Drawn> {
        self.drawn.as_ref()
    }

    fn kind(&self) -> &'static str {
        "build-steps"
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let lines = steps_lines(&self.shown, Instant::now(), &look.theme);
        let title = match self.shown {
            Shown::Failed { .. } => "build failed",
            _ => "restart",
        };
        let mut spec = BoxSpec::new(title, &lines);
        spec.keys = Some(FOOT);
        spec.place = Place::At {
            x: self.at.0,
            y: self.at.1,
        };
        spec.width = 0;
        spec.closes = false;
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    /// Keys go on to the tab, the box staying up.
    fn key(&mut self, _key: Key, _look: &Look) -> Step {
        Step::Through
    }

    fn lets_through(&self) -> bool {
        true
    }

    /// And so does what is pasted.
    fn paste(&mut self, _text: &str, _look: &Look) -> Step {
        Step::Through
    }

    /// A click anywhere closes it, and goes no further.
    fn mouse(&mut self, mouse: Mouse, _look: &Look) -> Step {
        match draw::gesture(&mouse) {
            Gesture::Click | Gesture::WheelUp | Gesture::WheelDown => {
                Step::Close(Outcome::Dismissed)
            }
            _ => Step::Same,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::input::{KeyCode, Mods};
    use crate::ui::draw::tests::screen;
    use ratatui::layout::Rect;

    const T: Theme = Theme::DARK;

    fn text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn building(now: Instant) -> Build {
        let mut b = Build::new(true, now);
        b.take(
            Report::Fetched {
                commits: Some(3),
                took: Duration::from_secs(2),
            },
            now,
        );
        b.take(
            Report::Building {
                dir: "/c/main".into(),
                head: "main at 1a2b3c4".into(),
                of: Some(415),
            },
            now,
        );
        b.take(Report::Built(212), now);
        b
    }

    #[test]
    fn the_row_reads_the_spinner_building_the_meter_and_the_time() {
        let now = Instant::now();
        let mut b = building(now);
        b.started = now - Duration::from_secs(41);
        let row = Shown::Building(b.clone()).row(now).unwrap();
        assert_eq!(
            row,
            Row::Working {
                what: "Building",
                meter: Some((212, 415)),
                secs: Some(41),
                note: ""
            }
        );
        let spans = row.spans(30, 2, &T);
        let line = text(&spans);
        assert_eq!(draw::width(&line), 30);
        assert!(line.starts_with(" ⠹ Building ▰▰▰▰▰▱▱▱▱▱"), "{line:?}");
        assert!(line.trim_end().ends_with("41s"), "{line:?}");
        assert!(row.spins());
        assert_eq!(row.words(2), "⠹ Building 212/~415 · 41s");
        // Narrow, the meter goes first.
        let narrow = text(&row.spans(16, 0, &T));
        assert!(!narrow.contains('▰'), "{narrow:?}");
        assert!(narrow.contains("41s"), "{narrow:?}");
        // The first time, no meter.
        b.of = None;
        let first = Shown::Building(b.clone()).row(now).unwrap();
        assert!(!text(&first.spans(30, 0, &T)).contains('▱'));
        b.restarting = true;
        let line = text(&Shown::Building(b).row(now).unwrap().spans(30, 0, &T));
        assert!(
            line.starts_with(" ⠋ Restarting  every pane kept"),
            "{line:?}"
        );
    }

    #[test]
    fn restarted_shows_its_hash_until_its_time_is_up_and_a_failure_stays() {
        let now = Instant::now();
        let restarted = Shown::Restarted {
            hash: "abc1234".into(),
            until: now + RESTARTED_FOR,
        };
        let row = restarted.row(now).unwrap();
        assert_eq!(
            text(&row.spans(28, 0, &T)).trim_end(),
            " ✓ Restarted on abc1234"
        );
        assert!(!row.spins());
        assert_eq!(restarted.row(now + RESTARTED_FOR), None);
        let failed = Shown::Failed {
            build: building(now),
            why: "could not compile".into(),
            lines: vec!["error[E0308]: mismatched types".into()],
        };
        let line = text(&failed.row(now).unwrap().spans(40, 0, &T));
        assert_eq!(line.trim_end(), " ✕ Build failed · click for why");
        assert_eq!(
            failed
                .row(now + Duration::from_secs(3600))
                .unwrap()
                .words(0),
            "✕ Build failed · click for why"
        );
    }

    #[test]
    fn the_steps_say_each_step_and_a_failure_its_error() {
        let now = Instant::now();
        let b = building(now);
        let lines: Vec<String> = steps_lines(&Shown::Building(b.clone()), now, &T)
            .iter()
            .map(|l| text(&l.spans))
            .collect();
        assert_eq!(lines[0], " main at 1a2b3c4");
        assert_eq!(lines[2], " ✓ Fetched  (3 commits, 2s)");
        assert!(
            lines[3].starts_with(" ⠋ Building  (212 of ~415, 0s)"),
            "{lines:?}"
        );
        assert_eq!(lines[4], " ○ Install  (~/.local/bin)");
        assert_eq!(lines[5], " ○ Restart  (panes kept)");
        assert!(lines[7].contains('▰'), "{lines:?}");
        let failed = Shown::Failed {
            build: b,
            why: "could not compile".into(),
            lines: vec![
                "error[E0308]: mismatched types".into(),
                "  --> src/x.rs:1:1".into(),
            ],
        };
        let lines: Vec<String> = steps_lines(&failed, now, &T)
            .iter()
            .map(|l| text(&l.spans))
            .collect();
        assert!(lines[3].starts_with(" ✕ Building"), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains('▰')));
        assert_eq!(lines[lines.len() - 2], " error[E0308]: mismatched types");
    }

    #[test]
    fn the_steps_box_sits_where_it_is_put_and_lets_keys_through() {
        let now = Instant::now();
        let mut steps = Steps::new(Shown::Building(building(now)), (21, 1));
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 30));
        steps.draw(&mut buf, &Look::default());
        let d = steps.drawn.clone().unwrap();
        assert_eq!((d.outer.x, d.outer.y), (21, 1));
        let s = screen(&buf);
        assert!(s.iter().any(|l| l.contains(FOOT)), "{s:?}");
        assert_eq!(
            steps.key(Key::new(KeyCode::Char('x'), Mods::NONE), &Look::default()),
            Step::Through
        );
        assert_eq!(steps.paste("pasted", &Look::default()), Step::Through);
        assert!(steps.stands_out().is_none(), "it dims nothing");
        assert!(steps.lets_through(), "the pane keeps its cursor");
    }
}
