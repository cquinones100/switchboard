//! The landings autopilot's tabs made without asking (its menu's
//! "Land without asking"), for the notifications: what queue-and-clean's
//! `switchboard land-lock autolanded <short hash> [--plan <file>] <item>` adds
//! once such a landing has pushed, `autolanded.json` in the state directory,
//! `[{ hash, item, tab, at, plan, decided }]`, `plan` the item's plan note
//! by its path within the notes directory and `decided` how many
//! `Decided hands-free` lines it holds (autopilot's hands-free mode), the last `KEPT`, each until it is dismissed. Ported
//! from autopilot.js's `readLanded` and `dismissLanded` and land-lock.js's
//! `autolanded`, the tab named by its id where tmux named its window.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::util;

/// The file, in the state directory.
pub const FILE: &str = "autolanded.json";
/// The landings kept.
pub const KEPT: usize = 20;

/// A landing made without asking.
#[derive(Clone, Debug, PartialEq)]
pub struct Landing {
    /// The commit pushed, short.
    pub hash: String,
    /// The item's first line; empty when it wasn't given.
    pub item: String,
    /// The tab it was made in, while it is open.
    pub tab: String,
    /// When (ms).
    pub at: i64,
    /// The item's plan note, by its path within the notes directory; empty
    /// for none.
    pub plan: String,
    /// The decisions its tab took hands-free, as its plan records them.
    pub decided: usize,
}

impl Landing {
    /// "Landed <hash>: <item>".
    pub fn title(&self) -> String {
        let item = if self.item.is_empty() {
            "a change"
        } else {
            &self.item
        };
        match self.decided {
            0 => format!("Landed {}: {item}", self.hash),
            1 => format!("Landed {}: {item} (1 decided hands-free)", self.hash),
            n => format!("Landed {}: {item} ({n} decided hands-free)", self.hash),
        }
    }
}

fn file(state: &Path) -> PathBuf {
    state.join(FILE)
}

/// The landings, oldest first; none when the file isn't there or isn't a
/// list.
pub fn read(state: &Path) -> Vec<Landing> {
    let Some(Value::Array(list)) = util::read_json(file(state)) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|l| {
            let text = |k: &str| l.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let hash = l.get("hash")?.as_str()?.to_string();
            Some(Landing {
                hash,
                item: text("item"),
                tab: text("tab"),
                at: l.get("at").and_then(Value::as_f64).unwrap_or(0.0) as i64,
                plan: text("plan"),
                decided: l.get("decided").and_then(Value::as_u64).unwrap_or(0) as usize,
            })
        })
        .collect()
}

fn write(state: &Path, list: &[Landing]) -> io::Result<()> {
    let value: Vec<Value> = list
        .iter()
        .map(|l| {
            json!({
                "hash": l.hash, "item": l.item, "tab": l.tab,
                "at": util::js_number(l.at as f64),
                "plan": l.plan, "decided": util::js_number(l.decided as f64),
            })
        })
        .collect();
    util::write_atomic(file(state), util::json_compact(&value))
}

/// How many decisions a plan note records as taken hands-free: its lines
/// holding `Decided hands-free`.
pub fn decided_in(plan: &str) -> usize {
    plan.lines()
        .filter(|l| l.contains("Decided hands-free"))
        .count()
}

/// A landing added, in place of one with the same hash, the oldest past
/// `KEPT` let go.
pub fn add(state: &Path, landing: Landing) -> io::Result<()> {
    let mut list: Vec<Landing> = read(state)
        .into_iter()
        .filter(|l| l.hash != landing.hash)
        .collect();
    list.push(landing);
    let over = list.len().saturating_sub(KEPT);
    write(state, &list[over..])
}

/// The landing with this hash put away, or every one.
pub fn dismiss(state: &Path, hash: Option<&str>) -> io::Result<()> {
    let list = read(state);
    let rest: Vec<Landing> = match hash {
        Some(hash) => list.into_iter().filter(|l| l.hash != hash).collect(),
        None => Vec::new(),
    };
    write(state, &rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landings_are_kept_newest_last_one_a_hash_until_dismissed() {
        let dir = std::env::temp_dir().join(format!("sb-autolanded-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read(&dir).is_empty());
        let landing = |hash: &str, item: &str| Landing {
            hash: hash.into(),
            item: item.into(),
            tab: "@3".into(),
            at: 1,
            plan: String::new(),
            decided: 0,
        };
        add(&dir, landing("abc1234", "Make the thing")).unwrap();
        add(&dir, landing("def5678", "")).unwrap();
        add(&dir, landing("abc1234", "Make the thing")).unwrap();
        let titles: Vec<String> = read(&dir).iter().map(Landing::title).collect();
        assert_eq!(
            titles,
            ["Landed def5678: a change", "Landed abc1234: Make the thing"]
        );
        for n in 0..KEPT {
            add(&dir, landing(&format!("{n:07}"), "x")).unwrap();
        }
        assert_eq!(read(&dir).len(), KEPT);
        add(
            &dir,
            Landing {
                plan: "Plans/Plan - X.md".into(),
                decided: decided_in(
                    "- Decided hands-free: A → B\n- other\n- Decided hands-free: C → D\n",
                ),
                ..landing("fff0000", "Make X")
            },
        )
        .unwrap();
        let last = read(&dir).pop().unwrap();
        assert_eq!(
            last.title(),
            "Landed fff0000: Make X (2 decided hands-free)"
        );
        assert_eq!(last.plan, "Plans/Plan - X.md");
        dismiss(&dir, Some("0000019")).unwrap();
        assert!(!read(&dir).iter().any(|l| l.hash == "0000019"));
        dismiss(&dir, None).unwrap();
        assert!(read(&dir).is_empty());
        std::fs::write(dir.join(FILE), "{}").unwrap();
        assert!(read(&dir).is_empty(), "a list or none");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
