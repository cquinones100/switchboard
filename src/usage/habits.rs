//! How you use the workspace, counted on this computer only and never sent
//! anywhere, for suggestions in the sidebar's notifications (the `habits`
//! setting): a view to turn off that you never open, a key for a command you
//! keep picking from M-p, and a feature that is off which what you do
//! suggests. Ported from lib/sidebar/habits.js.
//!
//! The counts are kept a file a day, `habits/<YYYY-MM-DD>.tsv` in the state
//! directory, each line `<kind>\t<name>\t<count>`, summed as read: `key` a
//! bound key pressed (its notation, `M-d`, or `M-b d` after the prefix),
//! `palette` a command picked in M-p (by its first root key, as palette.sh
//! pressed it, else `@palette <action>` for one with no key of its own) and
//! `view` a mission control view brought to the front. A root key picked in
//! M-p counts as pressed too, since tmux's palette pressed it; `pressed`
//! takes the larger of the two, so it counts once. Files over `KEEP_DAYS` old
//! are removed. What a suggestion's menu put off or away is kept in
//! `suggestions.json` beside them: an id and until when, 0 for good.
//!
//! The server adds up what is pressed in memory and writes it once a minute,
//! as the daemon took tmux's `@used_keys` once a minute, so a key press
//! never waits on a file. Every function here takes the state directory, so
//! the tests run in one of their own.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde_json::{Map, Value, json};

use crate::items::Item;
use crate::util;

/// Days of counts kept.
pub const KEEP_DAYS: i64 = 60;
pub const DAY_MS: i64 = 24 * 60 * 60_000;
/// How long "Not now" puts a suggestion off.
pub const NOT_NOW_MS: i64 = 14 * DAY_MS;
/// A view not opened in this many days is suggested off.
pub const UNUSED_DAYS: i64 = 30;
/// A command picked from M-p this many times in `PICK_DAYS` suggests its key.
pub const PICKS: u64 = 3;
pub const PICK_DAYS: i64 = 14;

fn habits_dir(state: &Path) -> PathBuf {
    state.join("habits")
}

fn dismissed_file(state: &Path) -> PathBuf {
    state.join("suggestions.json")
}

/// The local day an epoch time in milliseconds falls on, `YYYY-MM-DD`.
pub fn day_of(ms: i64) -> String {
    util::ymd(&util::local_at_ms(ms))
}

fn is_day_file(name: &str) -> bool {
    let b = name.as_bytes();
    name.len() == 14
        && name.ends_with(".tsv")
        && b[..10].iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// One count to add: a key pressed `n` times, a view brought forward.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Count {
    pub kind: String,
    pub name: String,
    pub n: u64,
}

impl Count {
    pub fn new(kind: &str, name: &str, n: u64) -> Count {
        Count {
            kind: kind.to_string(),
            name: name.to_string(),
            n,
        }
    }
}

/// Adds counts to the day `now` falls on. A count of 0, or a name holding a
/// tab or newline (which would break the line), is left out.
pub fn record(state: &Path, counts: &[Count], now: i64) {
    let lines: String = counts
        .iter()
        .filter(|c| c.n > 0 && !c.name.contains(['\t', '\n']) && !c.name.is_empty())
        .map(|c| format!("{}\t{}\t{}\n", c.kind, c.name, c.n))
        .collect();
    if lines.is_empty() {
        return;
    }
    let dir = habits_dir(state);
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    util::append_quietly(dir.join(format!("{}.tsv", day_of(now))), lines);
}

/// The days kept, oldest first, with the ones over `KEEP_DAYS` old removed.
pub fn prune(state: &Path, now: i64) -> Vec<String> {
    let dir = habits_dir(state);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| is_day_file(f))
        .collect();
    files.sort();
    let oldest = day_of(now - KEEP_DAYS * DAY_MS);
    files.retain(|f| {
        if f[..10] >= *oldest {
            return true;
        }
        let _ = fs::remove_file(dir.join(f));
        false
    });
    files
}

/// The counts of some days, by `<kind>\t<name>` in the order first seen, and
/// how many days back the counting started.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub counts: IndexMap<String, u64>,
    /// Days from the first one kept to today, both counted; 0 with none.
    pub tracked: i64,
}

impl Tally {
    /// A tally from counts given as they are kept, for the tests and the
    /// report's.
    pub fn of(counts: &[(&str, u64)], tracked: i64) -> Tally {
        Tally {
            counts: counts.iter().map(|(k, n)| (k.to_string(), *n)).collect(),
            tracked,
        }
    }

    pub fn count(&self, kind: &str, name: &str) -> u64 {
        self.counts
            .get(&format!("{kind}\t{name}"))
            .copied()
            .unwrap_or(0)
    }

    /// Each name of a kind with its count, in the order first seen.
    pub fn of_kind(&self, kind: &str) -> Vec<(String, u64)> {
        let prefix = format!("{kind}\t");
        self.counts
            .iter()
            .filter_map(|(k, n)| k.strip_prefix(&prefix).map(|name| (name.to_string(), *n)))
            .collect()
    }

    /// A key pressed, counting a pick in M-p that pressed it only once.
    pub fn pressed(&self, key: &str) -> u64 {
        self.count("key", key).max(self.count("palette", key))
    }
}

/// The counts of the last `span` days, today included.
pub fn tally(state: &Path, span: i64, now: i64) -> Tally {
    let files = prune(state, now);
    let from = day_of(now - (span - 1) * DAY_MS);
    let mut counts: IndexMap<String, u64> = IndexMap::new();
    for f in files.iter().filter(|f| f[..10] >= *from) {
        let text = fs::read_to_string(habits_dir(state).join(f)).unwrap_or_default();
        for line in text.split('\n') {
            let mut fields = line.split('\t');
            let (Some(kind), Some(name), Some(n)) = (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let Ok(n) = n.trim().parse::<u64>() else {
                continue;
            };
            if name.is_empty() || n == 0 {
                continue;
            }
            *counts.entry(format!("{kind}\t{name}")).or_insert(0) += n;
        }
    }
    let tracked = match files.first() {
        Some(first) => days_between(&first[..10], &day_of(now)).map_or(0, |d| d + 1),
        None => 0,
    };
    Tally { counts, tracked }
}

/// Whole days from one `YYYY-MM-DD` to another.
fn days_between(from: &str, to: &str) -> Option<i64> {
    let from: jiff::civil::Date = from.parse().ok()?;
    let to: jiff::civil::Date = to.parse().ok()?;
    Some(from.until(to).ok()?.get_days() as i64)
}

/// What a suggestion's menu put off (until when) or away (0).
pub fn read_dismissed(state: &Path) -> HashMap<String, i64> {
    util::read_json_object(dismissed_file(state))
        .into_iter()
        .filter_map(|(k, v)| Some((k, v.as_f64()? as i64)))
        .collect()
}

/// Puts a suggestion away for good, or off for `NOT_NOW_MS`.
pub fn dismiss(state: &Path, id: &str, for_good: bool, now: i64) {
    let mut saved: Map<String, Value> = util::read_json_object(dismissed_file(state));
    let until = if for_good { 0 } else { now + NOT_NOW_MS };
    saved.insert(id.to_string(), Value::from(until));
    let _ = fs::create_dir_all(state);
    let _ = util::write_atomic(
        dismissed_file(state),
        util::json_compact(&Value::Object(saved)),
    );
}

/// With the habits setting off, nothing counted is kept: the days and what
/// the suggestions' menus put off or away, so turning it on starts afresh.
pub fn forget(state: &Path) {
    let _ = fs::remove_dir_all(habits_dir(state));
    let _ = fs::remove_file(dismissed_file(state));
}

/// A mission control view that has a setting of its own, by its title; the
/// service it needs; and whether turning it off also stops what it looks up
/// in the background (the Calendar's feed and Slack's look don't wait on
/// their views).
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub title: &'static str,
    pub key: &'static str,
    pub needs: Option<&'static str>,
    pub lookups: bool,
}

const fn view(title: &'static str, key: &'static str) -> View {
    View {
        title,
        key,
        needs: None,
        lookups: false,
    }
}

const fn service_view(title: &'static str, key: &'static str, needs: &'static str) -> View {
    View {
        title,
        key,
        needs: Some(needs),
        lookups: true,
    }
}

pub const VIEWS: &[View] = &[
    view("Work map", "view_work_map"),
    view("Calendar", "view_calendar"),
    view("Slack", "view_slack"),
    view("Slack relay", "view_relay"),
    view("Closed", "view_closed"),
    view("Routines", "routines"),
    service_view("PRs", "pr_board", "github"),
    service_view("CI", "view_ci", "github"),
    View {
        title: "Landing",
        key: "view_landing",
        needs: None,
        lookups: true,
    },
    View {
        title: "Autopilot",
        key: "view_autopilot",
        needs: Some("autopilot"),
        lookups: false,
    },
    service_view("Tickets", "ticket_board", "linear"),
    service_view("Inbox", "view_inbox", "linear"),
    view("Sessions", "view_sessions"),
    view("Resources", "view_resources"),
    view("Usage", "usage"),
    view("Keys", "view_keys"),
];

/// A feature off (by default, or as an earlier install left it), suggested
/// once what you do within a week says it would help: `keys` pressed (or
/// picked in M-p) at least `times` together.
pub struct Try {
    pub key: &'static str,
    pub keys: &'static [&'static str],
    pub times: u64,
    pub title: &'static str,
    pub body: fn(u64) -> String,
}

// The keys are tmux.conf's, which the bindings keep; the commands that were
// palette-only there are counted by the names their actions take here
// (`@palette copy-pr` was tmux's `@palette p`), so they count once they are
// bound.
pub const TRIES: &[Try] = &[
    Try {
        key: "second_status",
        keys: &[
            "M-v",
            "M-i",
            "M-r",
            "M-l",
            "@palette copy-pr",
            "@palette copy-ticket",
        ],
        times: 10,
        title: "Turn on the second status line?",
        body: |n| {
            format!(
                "You looked up a tab's PR or ticket {n} times this week. The second status line shows the tab's branch, PR and Claude session under the tabs all the time."
            )
        },
    },
    Try {
        key: "todo_note",
        keys: &[
            "M-N",
            "M-e",
            "@palette choose-tab-note",
            "@palette note-tab",
        ],
        times: 10,
        title: "Turn on today's todo note?",
        body: |n| {
            format!(
                "You went to Notes {n} times this week. Each weekday morning a note Todos/Today's Todo <date> would gather your notes' unticked items, today's calendar, and your PRs, tickets and Slack. It sends your notes' unticked items to Claude (Sonnet) to write it."
            )
        },
    },
    // On by default; suggested to one who turned it off and keeps setting
    // reminders to come back to the same work.
    Try {
        key: "routines",
        keys: &["@palette remind-tab"],
        times: 5,
        title: "Turn Routines back on?",
        body: |n| {
            format!(
                "You set a reminder {n} times this week. A routine runs a prompt of yours on a schedule (weekdays at 8:45, every 30m 9-6) in a Claude tab of its own, its runs and their cost in mission control's Routines view."
            )
        },
    },
    // Off where an install made before the plugin kept Switchboard in
    // ~/.claude (src/setup/way.rs); turning it on moves it over.
    Try {
        key: "claude_plugin",
        keys: &["M-a", "M-c"],
        times: 20,
        title: "Load Switchboard as a plugin?",
        body: |n| {
            format!(
                "You went to a tab's Claude {n} times this week. As a plugin, Switchboard's hooks, skills and instructions reach only the Claude in its panes, and come out of ~/.claude, so Claude elsewhere has only your own; turning it on moves them over. It needs Claude Code 2.1.280 or later."
            )
        },
    },
];

/// The setting a suggestion changes, and its menu's words for doing it.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub key: String,
    pub value: Value,
    pub label: String,
}

/// One suggestion.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    /// `key:M-d`, `off:view_calendar`, `on:todo_note`.
    pub id: String,
    pub title: String,
    pub body: String,
    /// None for a key, which changes no setting.
    pub setting: Option<Change>,
}

impl Suggestion {
    /// As a notification: its title, its body as the detail, and the change
    /// it makes in its data.
    pub fn item(&self) -> Item {
        let setting = self
            .setting
            .as_ref()
            .map(|c| json!({ "key": c.key, "value": c.value, "label": c.label }));
        Item::new(self.id.clone(), self.title.clone())
            .detail(self.body.clone())
            .data(json!({ "kind": "habit", "setting": setting }))
    }
}

/// What a suggestion's menu does with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Make the change it suggests (and put it away for good).
    Apply,
    /// "Got it", for a key: put away for good.
    GotIt,
    /// Back in a fortnight.
    NotNow,
    /// Away for good.
    Never,
}

/// One option of a suggestion's menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub key: char,
    pub label: String,
    pub outcome: Outcome,
}

/// A suggestion's menu (the daemon's `noticeChoices` for a habit): what it
/// suggests, else put off or away. Each puts it away for good but "Not
/// now", which brings it back in a fortnight.
pub fn choices(s: &Suggestion) -> Vec<Choice> {
    let choice = |key, label: &str, outcome| Choice {
        key,
        label: label.to_string(),
        outcome,
    };
    match &s.setting {
        None => vec![
            choice('y', "Got it", Outcome::GotIt),
            choice('n', "Not now", Outcome::NotNow),
        ],
        Some(change) => vec![
            choice('y', &change.label, Outcome::Apply),
            choice('n', "Not now", Outcome::NotNow),
            choice('x', "Don't suggest it again", Outcome::Never),
        ],
    }
}

/// What the suggestions are made from: a month's, a fortnight's and a week's
/// counts, a setting's value (`None` for a key that isn't one), the root
/// keys' descriptions (key → what it does) and suggestions.json.
pub struct Inputs<'a> {
    pub month: &'a Tally,
    pub fortnight: &'a Tally,
    pub week: &'a Tally,
    pub value: &'a dyn Fn(&str) -> Option<Value>,
    pub notes: &'a IndexMap<String, String>,
    pub dismissed: &'a HashMap<String, i64>,
    pub now: i64,
}

/// Whether a setting is off, as `value(key) === false` read.
fn off(value: &dyn Fn(&str) -> Option<Value>, key: &str) -> bool {
    value(key) == Some(Value::Bool(false))
}

/// Whether a view is on, and its service with it.
pub fn view_on(v: &View, value: &dyn Fn(&str) -> Option<Value>) -> bool {
    !off(value, v.key) && v.needs.is_none_or(|n| !off(value, n))
}

/// A description without its `#tags`.
pub fn untagged(note: &str) -> &str {
    match note.find(" #") {
        Some(at) => note[..at].trim_end(),
        None => note,
    }
}

/// The suggestions to make now, from the counts alone, so its test needs
/// no server (`suggestionsFrom`): keys first, then features to try, then
/// views to turn off; each left out while put off or away.
pub fn suggestions_from(i: &Inputs) -> Vec<Suggestion> {
    let unused: Vec<Suggestion> = if i.month.tracked >= UNUSED_DAYS {
        VIEWS
            .iter()
            .filter(|v| view_on(v, i.value) && i.month.count("view", v.title) == 0)
            .map(|v| Suggestion {
                id: format!("off:{}", v.key),
                title: format!("Turn off the {} view?", v.title),
                body: format!(
                    "You haven't opened mission control's {} view in {UNUSED_DAYS} days.{} Settings turns it on again.",
                    v.title,
                    if v.lookups {
                        " Turning it off also stops what it looks up in the background."
                    } else {
                        ""
                    }
                ),
                setting: Some(Change {
                    key: v.key.to_string(),
                    value: Value::Bool(false),
                    label: "Turn it off".into(),
                }),
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut keys: Vec<(&String, &String, u64)> = i
        .notes
        .iter()
        .map(|(key, note)| (key, note, i.fortnight.count("palette", key)))
        .filter(|(_, _, n)| *n >= PICKS)
        .collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.2));
    let keys = keys.into_iter().map(|(key, note, n)| Suggestion {
        id: format!("key:{key}"),
        title: format!("{key}: {}", untagged(note)),
        body: format!(
            "You picked this from M-p {n} times in {PICK_DAYS} days. {key} does it in one press, from any pane."
        ),
        setting: None,
    });
    let tries = TRIES
        .iter()
        .filter(|t| off(i.value, t.key))
        .map(|t| (t, t.keys.iter().map(|k| i.week.pressed(k)).sum::<u64>()))
        .filter(|(t, n)| *n >= t.times)
        .map(|(t, n)| Suggestion {
            id: format!("on:{}", t.key),
            title: t.title.to_string(),
            body: (t.body)(n),
            setting: Some(Change {
                key: t.key.to_string(),
                value: Value::Bool(true),
                label: "Turn it on".into(),
            }),
        });
    keys.chain(tries)
        .chain(unused)
        .filter(|s| match i.dismissed.get(&s.id) {
            None => true,
            Some(&until) => until != 0 && until <= i.now,
        })
        .collect()
}

/// The suggestions now, from the files in `state`.
pub fn suggestions(
    state: &Path,
    value: &dyn Fn(&str) -> Option<Value>,
    notes: &IndexMap<String, String>,
    now: i64,
) -> Vec<Suggestion> {
    let month = tally(state, UNUSED_DAYS, now);
    let fortnight = tally(state, PICK_DAYS, now);
    let week = tally(state, 7, now);
    suggestions_from(&Inputs {
        month: &month,
        fortnight: &fortnight,
        week: &week,
        value,
        notes,
        dismissed: &read_dismissed(state),
        now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn temp_state(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sb-habits-{name}-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 2026-11-05 at noon, here.
    fn now() -> i64 {
        jiff::civil::date(2026, 11, 5)
            .at(12, 0, 0, 0)
            .to_zoned(jiff::tz::TimeZone::system())
            .unwrap()
            .timestamp()
            .as_millisecond()
    }

    fn on(_: &str) -> Option<Value> {
        Some(Value::Bool(true))
    }

    fn notes() -> IndexMap<String, String> {
        [
            ("M-d", "Open the branch's diff #git #diff"),
            ("M-a", "Go to the next tab that needs you #tabs"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    #[test]
    fn counts_are_summed_over_the_days_asked_for() {
        let state = temp_state("tally");
        let now = now();
        record(&state, &[Count::new("key", "M-d", 2)], now - 10 * DAY_MS);
        record(
            &state,
            &[
                Count::new("key", "M-d", 3),
                Count::new("view", "Calendar", 1),
            ],
            now,
        );
        record(&state, &[Count::new("key", "M-d", 1)], now);
        record(
            &state,
            &[
                Count::new("key", "bad\tname", 1),
                Count::new("key", "M-x", 0),
            ],
            now,
        );
        let week = tally(&state, 7, now);
        assert_eq!(week.count("key", "M-d"), 4);
        assert_eq!(week.count("view", "Calendar"), 1);
        assert_eq!(
            week.counts.len(),
            2,
            "a name with a tab, or a count of 0, is left out"
        );
        assert_eq!(week.tracked, 11, "counted since the first day kept");
        assert_eq!(tally(&state, 30, now).count("key", "M-d"), 6);

        record(&state, &[Count::new("key", "M-d", 1)], now - 90 * DAY_MS);
        tally(&state, 7, now);
        assert_eq!(
            fs::read_dir(state.join("habits")).unwrap().count(),
            2,
            "a day over 60 days old is removed"
        );
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn a_command_picked_three_times_suggests_its_key_and_each_can_be_put_off_or_away() {
        let state = temp_state("keys");
        let now = now();
        let views: Vec<(String, u64)> = VIEWS
            .iter()
            .map(|v| (format!("view\t{}", v.title), 1))
            .collect();
        let month = Tally {
            counts: views.into_iter().collect(),
            tracked: 40,
        };
        let fortnight = Tally::of(
            &[
                ("palette\tM-d", 3),
                ("palette\tM-a", 2),
                ("palette\t@palette reset-tips", 9),
            ],
            40,
        );
        let week = Tally::of(&[], 40);
        let notes = notes();
        let run = |dismissed: HashMap<String, i64>| {
            suggestions_from(&Inputs {
                month: &month,
                fortnight: &fortnight,
                week: &week,
                value: &on,
                notes: &notes,
                dismissed: &dismissed,
                now,
            })
        };
        let all = run(HashMap::new());
        assert_eq!(
            all.len(),
            1,
            "M-a picked twice and a palette-only command suggest nothing"
        );
        let key = &all[0];
        assert_eq!(key.id, "key:M-d");
        assert_eq!(key.title, "M-d: Open the branch's diff");
        assert!(key.body.contains("picked this from M-p 3 times"));
        assert_eq!(key.setting, None, "a key changes no setting");
        assert_eq!(
            choices(key).iter().map(|c| c.outcome).collect::<Vec<_>>(),
            vec![Outcome::GotIt, Outcome::NotNow]
        );

        let one = |id: &str, until: i64| HashMap::from([(id.to_string(), until)]);
        assert!(run(one("key:M-d", 0)).is_empty(), "put away for good");
        assert!(run(one("key:M-d", now + DAY_MS)).is_empty(), "put off");
        assert_eq!(
            run(one("key:M-d", now - 1)).len(),
            1,
            "back once put off long enough"
        );

        dismiss(&state, "key:M-d", false, now);
        dismiss(&state, "off:usage", true, now);
        let saved = util::read_json(state.join("suggestions.json")).unwrap();
        assert_eq!(
            saved,
            json!({ "key:M-d": now + 14 * DAY_MS, "off:usage": 0 })
        );
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn a_view_not_opened_in_30_days_is_suggested_off_while_it_and_its_service_are_on() {
        let now = now();
        let opened: Vec<(String, u64)> = VIEWS
            .iter()
            .filter(|v| !["Calendar", "PRs"].contains(&v.title))
            .map(|v| (format!("view\t{}", v.title), 2))
            .collect();
        let notes = notes();
        let none = HashMap::new();
        let run = |tracked: i64, value: &dyn Fn(&str) -> Option<Value>| {
            let month = Tally {
                counts: opened.clone().into_iter().collect(),
                tracked,
            };
            let empty = Tally::of(&[], tracked);
            suggestions_from(&Inputs {
                month: &month,
                fortnight: &empty,
                week: &empty,
                value,
                notes: &notes,
                dismissed: &none,
                now,
            })
        };
        assert!(run(29, &on).is_empty(), "not before 30 days are counted");
        let off = run(30, &on);
        assert_eq!(
            off.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["off:view_calendar", "off:pr_board"]
        );
        assert_eq!(
            off[0].setting,
            Some(Change {
                key: "view_calendar".into(),
                value: Value::Bool(false),
                label: "Turn it off".into()
            })
        );
        assert!(
            !off[0].body.contains("looks up"),
            "the Calendar's feed carries on with its view off"
        );
        assert!(
            off[1]
                .body
                .contains("stops what it looks up in the background")
        );
        let no_github = |k: &str| Some(Value::Bool(k != "github"));
        assert_eq!(
            run(30, &no_github)
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            vec!["off:view_calendar"],
            "the PRs view isn't shown with GitHub off"
        );
        let no_calendar = |k: &str| Some(Value::Bool(k != "view_calendar"));
        assert_eq!(
            run(30, &no_calendar)
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            vec!["off:pr_board"]
        );
    }

    #[test]
    fn a_feature_that_is_off_is_suggested_once_what_it_helps_with_is_done_often() {
        let now = now();
        let value = |k: &str| Some(Value::Bool(!["todo_note", "second_status"].contains(&k)));
        let empty = Tally::of(&[], 3);
        let notes = IndexMap::new();
        let none = HashMap::new();
        let run = |week: &[(&str, u64)], value: &dyn Fn(&str) -> Option<Value>| {
            let week = Tally::of(week, 3);
            suggestions_from(&Inputs {
                month: &empty,
                fortnight: &empty,
                week: &week,
                value,
                notes: &notes,
                dismissed: &none,
                now,
            })
        };
        assert!(run(&[("key\tM-N", 9)], &value).is_empty());
        // M-N picked in M-p is pressed by it too, so 6 presses and 4 picks are 6.
        assert!(run(&[("key\tM-N", 6), ("palette\tM-N", 4)], &value).is_empty());
        let todo = run(
            &[
                ("key\tM-N", 6),
                ("key\tM-e", 3),
                ("palette\t@palette choose-tab-note", 1),
            ],
            &value,
        );
        assert_eq!(todo[0].id, "on:todo_note");
        assert_eq!(
            todo[0].setting,
            Some(Change {
                key: "todo_note".into(),
                value: Value::Bool(true),
                label: "Turn it on".into()
            })
        );
        assert!(todo[0].body.contains("went to Notes 10 times"));
        assert!(
            todo[0]
                .body
                .contains("sends your notes' unticked items to Claude"),
            "says what it sends"
        );
        assert!(
            run(&[("key\tM-v", 10)], &on).is_empty(),
            "not once it is on"
        );
        assert_eq!(run(&[("key\tM-v", 10)], &value)[0].id, "on:second_status");
    }

    #[test]
    fn forget_removes_the_days_and_what_was_put_off_or_away() {
        let state = temp_state("forget");
        let now = now();
        record(&state, &[Count::new("key", "M-d", 1)], now);
        dismiss(&state, "key:M-d", true, now);
        forget(&state);
        assert!(!state.join("habits").exists());
        assert!(!state.join("suggestions.json").exists());
        assert_eq!(tally(&state, 30, now).tracked, 0);
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn tags_come_off_a_description() {
        assert_eq!(untagged("Open the diff #git #diff"), "Open the diff");
        assert_eq!(untagged("No tags"), "No tags");
        assert!(is_day_file("2026-11-05.tsv"));
        assert!(!is_day_file("2026-11-5.tsv"));
        assert!(!is_day_file("notes.tsv"));
    }
}
