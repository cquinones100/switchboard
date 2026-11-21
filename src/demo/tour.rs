//! `switchboard demo --tour <dir>`: the demo walked with a headless client
//! (src/ui/headless.rs), a picture saved of each board, so a Claude can
//! compare them with the design without anyone's screen. Each step is the
//! keys that reach a board from the workspace, the text that says it has
//! drawn, and the keys that leave it again; then mission control's groups
//! are walked with `}` and each group's views with `]`, by the names it
//! shows, one picture each, `<group>-<view>`; then the boxes its menus
//! open (`LAST`). Boxes a word of text reaches are typed into (`M-p`'s
//! search), and what only the mouse opens isn't in it.
//!
//! The boards show the demo's scripted Claudes partway through their
//! scripts (a question asked, a tool's result), which take them some twenty
//! seconds on an idle computer and far longer on a busy one, so the tour
//! first waits for them to get there (`ready`), and every wait is as long
//! as the computer is busy (`patience`). A wait that still fails saves the
//! screen it gave up on as that board's `.txt` and says what it showed.
//!
//! Each picture is `<dir>/<step>.png`, drawn by headless Chrome, with the
//! screen as text beside it (`<step>.txt`), which is all that is written
//! when Chrome isn't installed.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::ui::headless::{self, Client};

/// A board of the tour.
pub struct Step {
    pub name: &'static str,
    /// The keys that reach it from the workspace's first tab; a word
    /// `@wait:<text>` waits for the text instead (`press`), so a key
    /// meant for a box isn't sent before the box is open.
    pub keys: &'static [&'static str],
    /// Text on the screen once it has drawn.
    pub wait: &'static str,
    /// The keys that leave it.
    pub leave: &'static [&'static str],
}

pub const STEPS: &[Step] = &[
    Step {
        name: "workspace",
        keys: &["M-2"],
        wait: "└",
        leave: &[],
    },
    Step {
        name: "sidebar",
        keys: &["M-s"],
        wait: "Rate-limit",
        leave: &["Escape"],
    },
    Step {
        name: "tab-waiting",
        keys: &["M-3"],
        wait: "Do you want to proceed",
        leave: &[],
    },
    Step {
        name: "notes",
        keys: &["M-5"],
        wait: "Demo Notes",
        leave: &["M-2"],
    },
    Step {
        name: "diff",
        // M-d sent with M-2 can reach the Notes tab the step before left
        // in front, before the switch has landed: wait for tab 2's Claude,
        // whose statusline the Notes tab doesn't have.
        keys: &["M-2", "@wait:Demo model", "M-d"],
        wait: "feature.ts",
        leave: &["q"],
    },
    Step {
        name: "history",
        keys: &["M-2", "M-h"],
        wait: "move in time",
        leave: &["Escape"],
    },
    Step {
        name: "pr-view",
        keys: &["M-v"],
        wait: "Closes ABC-12",
        leave: &["M-c"],
    },
    Step {
        name: "ticket-view",
        keys: &["M-i"],
        wait: "opened it",
        leave: &["M-c"],
    },
    Step {
        name: "next-needing-you",
        keys: &["M-a"],
        wait: "Add a Python",
        leave: &["M-2"],
    },
    Step {
        name: "palette",
        keys: &["M-p"],
        wait: "Actions",
        leave: &["Escape"],
    },
    Step {
        name: "tab-picker",
        keys: &["M-j"],
        wait: "no PR",
        leave: &["Escape"],
    },
    Step {
        name: "sidebar-filtered",
        keys: &[
            "M-p",
            "sidebar to a repo",
            "@wait:Filter the sidebar to a repository",
            "Enter",
            // With no filter on, "Every repository" is the last row, which
            // a short list may not show: the prompt is there either way.
            "@wait:repository>",
            "api",
            "@wait:repository> api",
            "Enter",
        ],
        wait: "OTHER REPOS",
        leave: &[],
    },
    // The picker again, the filter on, cleared by typing once it shows.
    Step {
        name: "repository-picker",
        keys: &[
            // M-p right after the filter changes is lost; going to a tab first
            // takes it.
            "M-2",
            "M-p",
            "sidebar to a repo",
            "@wait:Filter the sidebar to a repository",
            "Enter",
        ],
        wait: "Every repository",
        leave: &[
            "Every",
            "@wait:repository> Every",
            "Enter",
            "@wait:10 tabs",
            // The first key after the picker closes is lost, as above.
            "M-2",
        ],
    },
    // Notes' list, its finder opened from Recent's header. A note item's
    // plan and its work box aren't in the tour: they need the cursor on a
    // given line, and Enter on the line before it ticks an item.
    Step {
        name: "notes-finder",
        keys: &["M-5", "@wait:Demo Notes", "M-Left", "g", "g", "Enter"],
        wait: "Recent notes:",
        leave: &["Escape", "M-Right", "M-2"],
    },
];

/// The boxes mission control's menus open, walked after its views, each
/// from mission control opened afresh (on Tabs) and closed again. Several
/// views at once comes last, since showing them changes what mission
/// control opens on.
pub const LAST: &[Step] = &[
    Step {
        name: "item-menu",
        keys: &["M-Space", "@wait:MISSION CONTROL", "Enter"],
        wait: "Go to tab",
        leave: &["Escape", "Escape"],
    },
    Step {
        name: "reminder-box",
        keys: &[
            "M-Space",
            "@wait:MISSION CONTROL",
            "Enter",
            "@wait:Go to tab",
            "/",
            "Remind",
            "@wait:Remind me about it later",
            "Enter",
        ],
        wait: "WHEN",
        leave: &["Escape", "Escape", "Escape"],
    },
    Step {
        name: "view-menu",
        keys: &["M-Space", "@wait:MISSION CONTROL", "m"],
        wait: "view actions",
        leave: &["Escape", "Escape"],
    },
    Step {
        name: "find-key",
        keys: &[
            // M-/ opens the Keys view itself: M-[ (Escape, then [) followed by [
            // can be read as one escape sequence.
            "M-/",
            "@wait:WORKSPACE",
            "m",
            "@wait:view actions",
            "Enter",
        ],
        wait: "Press a key",
        leave: &["Escape", "Escape", "Escape"],
    },
    Step {
        name: "autopilot-settings",
        keys: &[
            "M-Space",
            "@wait:MISSION CONTROL",
            "]",
            "]",
            "m",
            "@wait:view actions",
            "Enter",
        ],
        wait: "another note",
        leave: &["Escape", "Escape", "Escape"],
    },
    Step {
        name: "several-views-box",
        keys: &[
            "M-Space",
            "@wait:MISSION CONTROL",
            "m",
            "@wait:view actions",
            "Enter",
        ],
        wait: "LAYOUT",
        leave: &[],
    },
    Step {
        name: "several-views",
        keys: &["Enter"],
        wait: "views at once",
        leave: &["Escape", "Escape"],
    },
];

/// How long a step waits for its board, or a box, to show on an idle
/// computer, times `patience` on a busy one, since a step that misses it
/// fails the tour.
const WAIT: Duration = Duration::from_secs(20);

/// How long the scripted Claudes get to reach their questions on an idle
/// computer (`ready`), times `patience`: the slower of the two asks after
/// 18 seconds of its script's own pauses and a dozen hooks, each a process.
const READY: Duration = Duration::from_secs(60);

/// The tabs whose scripted Claude a board shows waiting on you
/// (src/demo/agent.rs's `dark-mode` and `api-guide`), by the names
/// `demo::tabs` opens them with. The others' boards show what theirs drew
/// well before these ask.
const WAITING: &[&str] = &["Dark mode toggle", "API quickstart guide"];

/// The most of a screen a failed wait prints.
const SHOWN: usize = 15;

/// How many times longer than on an idle computer the tour waits for
/// anything: the one-minute load average over the cores, rounded up, at
/// least 1 and at most 8, read at each wait, as the tests' waits are
/// (tests/common's `patience`), so a busy computer makes the tour slower
/// rather than failed.
pub(crate) fn patience() -> u32 {
    let mut load = [0f64; 3];
    // SAFETY: getloadavg writes at most the 3 samples asked for.
    let read = unsafe { libc::getloadavg(load.as_mut_ptr(), 3) };
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    patience_from(if read > 0 { load[0] } else { 0.0 }, cores)
}

fn patience_from(load: f64, cores: usize) -> u32 {
    ((load / cores.max(1) as f64).ceil() as u32).clamp(1, 8)
}

/// Waits until the Claude in each of `WAITING`'s tabs is waiting on you,
/// by what the server says of its tabs (`list`), so no key is pressed, and
/// no board waited for, while the scripts are still on their way there.
fn ready(home: &Path) -> Result<()> {
    let deadline = Instant::now() + READY * patience();
    loop {
        let listed: Value = serde_json::from_str(&super::ask(home, &["list"])?)
            .context("the demo's server listed its tabs as something other than JSON")?;
        let behind = not_waiting(&listed);
        if behind.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "the demo's Claude never asked its question in {}",
                behind.join(" or ")
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The tabs of `WAITING` whose Claude isn't waiting on you, in `list`'s
/// answer.
fn not_waiting(listed: &Value) -> Vec<&'static str> {
    let tabs: Vec<&Value> = listed["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["tabs"].as_array().into_iter().flatten())
        .collect();
    WAITING
        .iter()
        .copied()
        .filter(|name| {
            !tabs
                .iter()
                .any(|t| t["name"] == *name && t["agent"] == "waiting")
        })
        .collect()
}

/// The most groups, and views in a group, the tour walks.
const MOST: usize = 12;

/// Mission control's groups, each with its views, read off the list of
/// them (src/mission/nav.rs) under its MISSION CONTROL label: in the
/// sidebar's place, or in a column of its own beside the tabs sidebar. A
/// group is a label in capitals one column in; a view a row two columns in
/// past the chosen row's bar, its name ending where its fact is set apart.
/// A line between columns may be a scroll bar's thumb.
pub fn mission_names(lines: &[String]) -> Vec<(String, Vec<String>)> {
    let Some(column) = mission_column(lines) else {
        return Vec::new();
    };
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let mut started = false;
    for line in lines {
        let part = line.split(['│', '┃']).nth(column).unwrap_or("");
        let trimmed = part.trim();
        if trimmed.starts_with("MISSION CONTROL") {
            started = true;
            continue;
        }
        if !started || trimmed.is_empty() {
            continue;
        }
        let label = part.starts_with(' ')
            && !part.starts_with("  ")
            && trimmed.chars().all(|c| c.is_uppercase() || c == ' ');
        if label {
            out.push((trimmed.to_string(), Vec::new()));
        } else if let Some(row) = part
            .strip_prefix("▌  ")
            .or_else(|| part.strip_prefix("   "))
        {
            let name = row.split("  ").next().unwrap_or("").trim();
            if let Some((_, views)) = out.last_mut()
                && !name.is_empty()
            {
                views.push(name.to_string());
            }
        }
    }
    out
}

/// Which of a line's parts, split at the lines between columns, holds
/// mission control's list: the one its MISSION CONTROL label is in.
fn mission_column(lines: &[String]) -> Option<usize> {
    lines.iter().find_map(|l| {
        l.split(['│', '┃'])
            .position(|part| part.trim_start().starts_with("MISSION CONTROL"))
    })
}

/// The view mission control's list marks as chosen, the row with the bar,
/// by its name as `mission_names` reads it.
pub fn chosen_view(lines: &[String]) -> Option<String> {
    let column = mission_column(lines)?;
    lines.iter().find_map(|line| {
        let row = line.split(['│', '┃']).nth(column)?.strip_prefix("▌  ")?;
        let name = row.split("  ").next().unwrap_or("").trim();
        (!name.is_empty()).then(|| name.to_string())
    })
}

fn slug(name: &str) -> String {
    name.to_lowercase().replace(' ', "-")
}

/// Saves the screen as `<dir>/<name>.txt` and, with Chrome, `.png`; gives
/// whether the PNG was drawn.
fn save(client: &Client, dir: &Path, name: &str) -> Result<bool> {
    let text = client.read(|s| s.text().join("\n"));
    crate::util::write_atomic(dir.join(format!("{name}.txt")), text)?;
    Ok(client
        .read(|s| headless::png(s, &dir.join(format!("{name}.png"))))
        .is_ok())
}

/// What a wait that failed says: `what` never happened, where the screen
/// it gave up on was saved, and the last `SHOWN` lines of it that hold
/// anything, so the failure names its cause.
fn gave_up(what: &str, saved: &Path, screen: &[String]) -> String {
    let held: Vec<&str> = screen
        .iter()
        .map(|l| l.trim_end())
        .filter(|l| !l.trim().is_empty())
        .collect();
    let last = &held[held.len().saturating_sub(SHOWN)..];
    format!(
        "{what}; the screen then ({}) ended:\n{}",
        saved.display(),
        last.join("\n")
    )
}

/// Fails a wait on the board `name`: saves the screen as its `.txt`, and
/// gives `gave_up`'s words, or why the server ended the client.
fn fail(client: &Client, dir: &Path, name: &str, what: &str) -> anyhow::Error {
    let screen = client.read(|s| s.text());
    let file = dir.join(format!("{name}.txt"));
    let _ = crate::util::write_atomic(&file, screen.join("\n"));
    match client.ended() {
        Some(why) => anyhow::anyhow!("{what}: {why}"),
        None => anyhow::anyhow!(gave_up(what, &file, &screen)),
    }
}

/// Waits for `text` on the board `name`, at most `WAIT` times `patience`.
fn shown(client: &Client, dir: &Path, name: &str, text: &str) -> Result<()> {
    if client.wait_text(text, WAIT * patience()) {
        return Ok(());
    }
    Err(fail(client, dir, name, &format!("\"{text}\" never showed")))
}

/// Presses `key` and waits, at most 10 seconds times `patience`, until
/// mission control's list has `view` chosen, so the board saved next is
/// that view's: a screen that only changed says nothing of the key, since
/// a view's clock or a tab's spinner changes it too. Fails when the view
/// is never chosen, the screen saved as the board `name`.
fn press_to_view(client: &mut Client, dir: &Path, name: &str, key: &str, view: &str) -> Result<()> {
    client.keys(&words(&[key]))?;
    let deadline = Instant::now() + Duration::from_secs(10) * patience();
    while client.read(|s| chosen_view(&s.lines())).as_deref() != Some(view) {
        if Instant::now() >= deadline || client.ended().is_some() {
            let what = format!("{key} never chose {view} in mission control's list");
            return Err(fail(client, dir, name, &what));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    loaded(client, dir, name)
}

/// What mission control draws in a view whose data hasn't come yet
/// (src/mission/render.rs), a row of the view's alone.
const LOADING: &str = "Loading…";

/// Whether a screen's line has the loading row in one of its panes, as a
/// row of its own rather than in a name that happens to hold the word.
fn placeholder(line: &str) -> bool {
    line.split('│').any(|part| part.trim() == LOADING)
}

/// Waits, at most `WAIT` times `patience`, until the view in front no
/// longer has `LOADING` as a row of its own (`placeholder`), then for its
/// screen to settle, so the board saved next has the view's data rather than its placeholder: the list moves
/// before the view beside it has drawn, and a view's lookup can take
/// seconds on a busy computer. Fails when it never loads, the screen saved
/// as the board `name`.
fn loaded(client: &Client, dir: &Path, name: &str) -> Result<()> {
    let deadline = Instant::now() + WAIT * patience();
    loop {
        client.settle();
        if !client.read(|s| s.lines().iter().any(|l| placeholder(l))) {
            return Ok(());
        }
        if Instant::now() >= deadline || client.ended().is_some() {
            return Err(fail(client, dir, name, "the view never loaded"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn words(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|k| k.to_string()).collect()
}

/// Takes each of `steps` in turn, saving its board; gives whether a PNG
/// was drawn, and fails at a step whose board never shows its text, rather
/// than saving another screen in its name.
fn walk(client: &mut Client, dir: &Path, steps: &[Step], saved: &mut Vec<String>) -> Result<bool> {
    let mut drew_png = false;
    for step in steps {
        let board = || format!("the {} board", step.name);
        press(client, dir, step.name, step.keys).with_context(board)?;
        if !step.wait.is_empty() {
            shown(client, dir, step.name, step.wait).with_context(board)?;
        }
        drew_png |= save(client, dir, step.name)?;
        saved.push(step.name.to_string());
        // What a failed wait saves is the screen on the way out, not the
        // board's own, already saved.
        let leaving = format!("{}-leaving", step.name);
        press(client, dir, &leaving, step.leave)
            .with_context(|| format!("leaving the {} board", step.name))?;
    }
    Ok(drew_png)
}

/// Escape as kitty's keyboard protocol sends it, never part of another key.
const ESCAPE: &str = "\x1b[27u";

/// Sends `keys`, waiting at each `@wait:<text>` (`shown`, as the board
/// `name`) for the text to show, and failing when it doesn't.
fn press(client: &mut Client, dir: &Path, name: &str, keys: &[&str]) -> Result<()> {
    for key in keys {
        match key.strip_prefix("@wait:") {
            // A box that never opened would take the next keys meant for it
            // somewhere else, so the tour stops there.
            Some(text) => shown(client, dir, name, text)?,
            // A bare Escape followed at once by the next key's bytes (an
            // Escape, or M-/'s Escape and /) can be read as one Alt key, so
            // it is sent in the form that can't (kitty's CSI u), which the
            // server reads as Escape.
            None if *key == "Escape" => client.keys(&words(&[ESCAPE]))?,
            None => client.keys(&words(&[key]))?,
        }
    }
    Ok(())
}

/// Walks the tour on the server of the demo in `home`, once its Claudes
/// are where the boards show them, saving into `dir`; gives the names
/// saved.
pub fn run(home: &Path, dir: &Path, cols: u16, rows: u16) -> Result<Vec<String>> {
    fs::create_dir_all(dir).with_context(|| format!("couldn't make {}", dir.display()))?;
    ready(home)?;
    let mut client = Client::attach(&super::socket_of(home), cols, rows, None, headless::TERM)?;
    let mut saved = Vec::new();
    let mut drew_png = false;
    drew_png |= walk(&mut client, dir, STEPS, &mut saved)?;
    client.keys(&words(&["M-Space"]))?;
    shown(&client, dir, "mission-control", "MISSION CONTROL").context("mission control")?;
    let groups = client.read(|s| mission_names(&s.lines()));
    for (g, (group, views)) in groups.iter().take(MOST).enumerate() {
        for (v, view) in views.iter().take(MOST).enumerate() {
            let name = format!("{}-{}", slug(group), slug(view));
            match (g, v) {
                // Mission control opens on the first group's first view.
                (0, 0) => loaded(&client, dir, &name)?,
                // The next group's first view.
                (_, 0) => press_to_view(&mut client, dir, &name, "M-]", view)?,
                _ => press_to_view(&mut client, dir, &name, "]", view)?,
            }
            drew_png |= save(&client, dir, &name)?;
            saved.push(name);
        }
    }
    client.keys(&words(&["q"]))?;
    drew_png |= walk(&mut client, dir, LAST, &mut saved)?;
    client.detach();
    if !drew_png {
        eprintln!(
            "switchboard demo: no Chrome to draw pictures with, so only the screens' text was saved"
        );
    }
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_row_of_its_own_says_a_view_is_loading() {
        assert!(placeholder(" MISSION CONTROL   │  Loading…"));
        assert!(!placeholder(" │ ○ 1 Rate-limit Loading… · idle"));
    }

    #[test]
    fn a_busy_computer_waits_longer_within_bounds() {
        assert_eq!(patience_from(0.0, 8), 1);
        assert_eq!(patience_from(7.9, 8), 1);
        assert_eq!(patience_from(12.0, 8), 2);
        assert_eq!(patience_from(400.0, 8), 8);
        assert_eq!(patience_from(3.0, 0), 3);
    }

    #[test]
    fn the_tour_waits_for_each_claude_that_a_board_shows_asking() {
        let tab = |name: &str, agent: Value| serde_json::json!({ "name": name, "agent": agent });
        let listed = |dark: Value, guide: Value| {
            serde_json::json!({ "sessions": [{ "tabs": [
                tab("code", Value::Null),
                tab("Dark mode toggle", dark),
                tab("API quickstart guide", guide),
            ] }] })
        };
        assert_eq!(
            not_waiting(&listed("working".into(), "waiting".into())),
            ["Dark mode toggle"]
        );
        assert!(not_waiting(&listed("waiting".into(), "waiting".into())).is_empty());
        // A server with no tabs yet has neither.
        assert_eq!(not_waiting(&serde_json::json!({})), WAITING);
    }

    #[test]
    fn a_failed_wait_says_what_the_screen_showed() {
        let mut screen: Vec<String> = (1..=20).map(|n| format!(" line {n}   ")).collect();
        screen.insert(10, "   ".into());
        screen.push(String::new());
        let said = gave_up(
            "\"Do you want to proceed\" never showed",
            Path::new("/tour/tab-waiting.txt"),
            &screen,
        );
        let mut lines = said.lines();
        assert_eq!(
            lines.next(),
            Some(
                "\"Do you want to proceed\" never showed; the screen then (/tour/tab-waiting.txt) ended:"
            )
        );
        // Its last lines that hold anything, as they were but for the
        // blanks after them.
        let shown: Vec<&str> = lines.collect();
        assert_eq!(shown.len(), SHOWN);
        assert_eq!(shown.first(), Some(&" line 6"));
        assert_eq!(shown.last(), Some(&" line 20"));
    }

    #[test]
    fn mission_controls_groups_and_views_are_read_off_its_list() {
        let lines: Vec<String> = [
            " ‹ Tabs  ·  2 need you   / filter │ Tickets   Code",
            "                                  │─────────",
            " MISSION CONTROL                  │",
            "                                  │",
            " WORK                             │",
            "   Tabs                2 need you │",
            "   Map                         14 │",
            "                                  │",
            " CODE                             │",
            "   PRs                          5 │",
            "▌  Tickets                      3 │",
            "                                  │",
            " j/k view  Enter open  Esc tabs   │",
        ]
        .iter()
        .map(|l| l.to_string())
        .collect();
        let names = mission_names(&lines);
        assert_eq!(
            names,
            [
                (
                    "WORK".to_string(),
                    vec!["Tabs".to_string(), "Map".to_string()]
                ),
                (
                    "CODE".to_string(),
                    vec!["PRs".to_string(), "Tickets".to_string()]
                ),
            ]
        );

        // Its own column, beside the tabs sidebar.
        let column: Vec<String> = [
            " 2 tabs   │ MISSION CONTROL       │ Tickets   Code",
            " NEEDS YOU│                       │",
            " ● 1 api  │ CODE                  │",
            "          │▌  Tickets           3 │",
        ]
        .iter()
        .map(|l| l.to_string())
        .collect();
        assert_eq!(
            mission_names(&column),
            [("CODE".to_string(), vec!["Tickets".to_string()])]
        );

        // The chosen view is the row with the bar, in either place.
        assert_eq!(chosen_view(&lines).as_deref(), Some("Tickets"));
        assert_eq!(chosen_view(&column).as_deref(), Some("Tickets"));
        assert_eq!(chosen_view(&lines[..6]), None);
    }
}
