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
//! Each picture is `<dir>/<step>.png`, drawn by headless Chrome, with the
//! screen as text beside it (`<step>.txt`), which is all that is written
//! when Chrome isn't installed.

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::ui::headless::{self, Client};

const CAP_LEFT: char = '\u{E0B6}';
const CAP_RIGHT: char = '\u{E0B4}';

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
        keys: &["M-2", "M-d"],
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
            "@wait:Every repository",
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

/// How long a step waits for its board, or a box, to show: long enough for
/// a busy computer, since a step that misses it fails the tour.
const WAIT: Duration = Duration::from_secs(20);

/// The most groups, and views in a group, the tour walks.
const MOST: usize = 12;

/// Mission control's groups (its first line) and the views of the group in
/// front (its second), read off the screen right of the sidebar, each a
/// name set apart from the next by three spaces or more, the chosen one's
/// half circles (drawn for Ghostty) read as spaces.
pub fn mission_names(lines: &[String]) -> (Vec<String>, Vec<String>) {
    let names = |line: Option<&String>| -> Vec<String> {
        let line = line
            .map(String::as_str)
            .unwrap_or("")
            .replace([CAP_LEFT, CAP_RIGHT], " ");
        let line = line
            .split_once('│')
            .map_or(line.as_str(), |(_, right)| right);
        line.split("   ")
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .take_while(|w| !w.starts_with('/') && !w.starts_with('[') && !w.starts_with("M-["))
            .filter(|w| w.chars().all(|c| c.is_alphanumeric() || c == ' '))
            .map(str::to_string)
            .collect()
    };
    let groups = names(lines.first())
        .into_iter()
        .filter(|g| g != "MISSION CONTROL")
        .collect();
    (groups, names(lines.get(1)))
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

/// Presses `key` and waits, at most 10 seconds, until `read` gives
/// something other than it did before, so what is read next is what the key
/// drew, not the screen before it; fails when it never does.
fn press_until<T: PartialEq>(
    client: &mut Client,
    key: &str,
    read: impl Fn(&headless::Screen) -> T,
) -> Result<()> {
    let before = client.read(&read);
    client.keys(&words(&[key]))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while client.read(&read) == before {
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("the screen didn't change after {key}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
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
        press(client, step.keys).with_context(|| format!("the {} board", step.name))?;
        if !step.wait.is_empty() && !client.wait_text(step.wait, WAIT) {
            anyhow::bail!("the {} board never showed \"{}\"", step.name, step.wait);
        }
        drew_png |= save(client, dir, step.name)?;
        saved.push(step.name.to_string());
        press(client, step.leave).with_context(|| format!("leaving the {} board", step.name))?;
    }
    Ok(drew_png)
}

/// Sends `keys`, waiting at each `@wait:<text>` (at most `WAIT`) for
/// the text to show, and failing when it doesn't.
fn press(client: &mut Client, keys: &[&str]) -> Result<()> {
    for key in keys {
        match key.strip_prefix("@wait:") {
            // A box that never opened would take the next keys meant for it
            // somewhere else, so the tour stops there.
            Some(text) => {
                if !client.wait_text(text, WAIT) {
                    anyhow::bail!("\"{text}\" never showed");
                }
            }
            None => client.keys(&words(&[key]))?,
        }
    }
    Ok(())
}

/// Walks the tour on the server on `server`, saving into `dir`; gives the
/// names saved.
pub fn run(server: &Path, dir: &Path, cols: u16, rows: u16) -> Result<Vec<String>> {
    fs::create_dir_all(dir).with_context(|| format!("couldn't make {}", dir.display()))?;
    let mut client = Client::attach(server, cols, rows, None, headless::TERM)?;
    let mut saved = Vec::new();
    let mut drew_png = false;
    drew_png |= walk(&mut client, dir, STEPS, &mut saved)?;
    client.keys(&words(&["M-Space"]))?;
    if !client.wait_text("MISSION CONTROL", Duration::from_secs(10)) {
        anyhow::bail!("mission control didn't open");
    }
    let (groups, _) = client.read(|s| mission_names(&s.lines()));
    for (g, group) in groups.iter().take(MOST).enumerate() {
        if g > 0 {
            // The next group's views in place of this one's.
            press_until(&mut client, "M-]", |s| mission_names(&s.lines()))?;
        }
        let (_, views) = client.read(|s| mission_names(&s.lines()));
        for (v, view) in views.iter().take(MOST).enumerate() {
            if v > 0 {
                press_until(&mut client, "]", |s| s.text())?;
            }
            let name = format!("{}-{}", slug(group), slug(view));
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
    fn mission_controls_groups_and_views_are_read_off_its_first_lines() {
        let lines: Vec<String> = [
            " 5 tabs     / filter │ MISSION CONTROL   Work   Code   Day   System   ✗ 1  ! 2  ✓ 1",
            "                     │  PRs   Tickets   CI     / filter      M-[ M-] groups  [ ] views",
        ]
        .iter()
        .map(|l| l.to_string())
        .collect();
        let (groups, views) = mission_names(&lines);
        assert_eq!(groups, ["Work", "Code", "Day", "System"]);
        assert_eq!(views, ["PRs", "Tickets", "CI"]);

        let chosen: Vec<String> = [
            " 5 tabs     / filter │ MISSION CONTROL   \u{E0B6}Work\u{E0B4}   Code   Day   System",
            "                     │ PRs   \u{E0B6}Tickets\u{E0B4}   CI   M-[ M-] groups",
        ]
        .iter()
        .map(|l| l.to_string())
        .collect();
        let (groups, views) = mission_names(&chosen);
        assert_eq!(groups, ["Work", "Code", "Day", "System"]);
        assert_eq!(views, ["PRs", "Tickets", "CI"]);
    }
}
