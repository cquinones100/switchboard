//! `switchboard demo --tour <dir>`: the demo walked with a headless client
//! (src/ui/headless.rs), a picture saved of each board, so a Claude can
//! compare them with the design without anyone's screen. Each step is the
//! keys that reach a board from the workspace, the text that says it has
//! drawn, and the keys that leave it again; then mission control's groups
//! are walked with `}` and each group's views with `]`, by the names it
//! shows, one picture each, `<group>-<view>`.
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
    /// The keys that reach it from the workspace's first tab.
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
        wait: "⎿",
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
];

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
            .take_while(|w| !w.starts_with('/') && !w.starts_with('{'))
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

/// Walks the tour on the server on `server`, saving into `dir`; gives the
/// names saved.
pub fn run(server: &Path, dir: &Path, cols: u16, rows: u16) -> Result<Vec<String>> {
    fs::create_dir_all(dir).with_context(|| format!("couldn't make {}", dir.display()))?;
    let mut client = Client::attach(server, cols, rows, None, headless::TERM)?;
    let mut saved = Vec::new();
    let mut drew_png = false;
    for step in STEPS {
        client.keys(&words(step.keys))?;
        if !step.wait.is_empty() {
            client.wait_text(step.wait, Duration::from_secs(10));
        }
        drew_png |= save(&client, dir, step.name)?;
        saved.push(step.name.to_string());
        client.keys(&words(step.leave))?;
    }
    client.keys(&words(&["M-Space"]))?;
    if !client.wait_text("MISSION CONTROL", Duration::from_secs(10)) {
        anyhow::bail!("mission control didn't open");
    }
    let (groups, _) = client.read(|s| mission_names(&s.lines()));
    for (g, group) in groups.iter().take(MOST).enumerate() {
        if g > 0 {
            // The next group's views in place of this one's.
            press_until(&mut client, "}", |s| mission_names(&s.lines()))?;
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
            "                     │  PRs   Tickets   CI     / filter        { } groups  [ ] views",
        ]
        .iter()
        .map(|l| l.to_string())
        .collect();
        let (groups, views) = mission_names(&lines);
        assert_eq!(groups, ["Work", "Code", "Day", "System"]);
        assert_eq!(views, ["PRs", "Tickets", "CI"]);

        let chosen: Vec<String> = [
            " 5 tabs     / filter │ MISSION CONTROL   \u{E0B6}Work\u{E0B4}   Code   Day   System",
            "                     │ PRs   \u{E0B6}Tickets\u{E0B4}   CI     { } groups",
        ]
        .iter()
        .map(|l| l.to_string())
        .collect();
        let (groups, views) = mission_names(&chosen);
        assert_eq!(groups, ["Work", "Code", "Day", "System"]);
        assert_eq!(views, ["PRs", "Tickets", "CI"]);
    }
}
