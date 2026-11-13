//! Which of the relay's messages reach you, and how: the one place a
//! message's destination is decided. New with the Slack relay view, with
//! no tmux-version file behind it.
//!
//! Everything the relay says by itself about a tab is of a `Kind`, and
//! each kind has a setting in the "Slack relay" category
//! (`relay_send_<kind>`): Notify (the tab's thread and a line in the main
//! DM, which is what Slack notifies you for), Thread (the thread alone) or
//! Off (nothing posted, though the message still counts as reported, so
//! turning the kind back on doesn't post a backlog). `relay_send_autopilot`
//! keeps a tab autopilot opened to its prompts, dialogs and failures, and
//! quiet hours turn Notify into Thread by the computer's clock. The
//! settings are read as each message goes out, so a change takes effect
//! with the next one, the relay running or not. Offline and crash messages
//! have no kind, and an answer to what you sent a tab from Slack goes
//! through the follow-ups (src/relay/followups.rs), which never ask here.
//!
//! Each message sent is a line in `sent/<day>.tsv` in the relay's state,
//! which mission control's Slack relay view counts.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::util;

/// What the relay says by itself, by its setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Prompts,
    Dialogs,
    Finished,
    Stuck,
    Milestones,
    Failures,
    Context,
    Notices,
}

impl Kind {
    pub const ALL: [Kind; 8] = [
        Kind::Prompts,
        Kind::Dialogs,
        Kind::Finished,
        Kind::Stuck,
        Kind::Milestones,
        Kind::Failures,
        Kind::Context,
        Kind::Notices,
    ];

    /// Its name in the sent file and its setting's key.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Prompts => "prompts",
            Kind::Dialogs => "dialogs",
            Kind::Finished => "finished",
            Kind::Stuck => "stuck",
            Kind::Milestones => "milestones",
            Kind::Failures => "failures",
            Kind::Context => "context",
            Kind::Notices => "notices",
        }
    }

    pub fn of_name(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == name)
    }

    pub fn key(self) -> String {
        format!("relay_send_{}", self.name())
    }

    /// What the relay did before the setting existed, its default.
    pub fn default(self) -> Send {
        match self {
            Kind::Finished | Kind::Milestones | Kind::Context => Send::Thread,
            _ => Send::Notify,
        }
    }

    /// The choices its setting offers: a prompt or dialog can't be turned
    /// off, since only Slack may be there to answer it, and a notice is a
    /// message of its own, with no thread to go to.
    pub fn choices(self) -> &'static [Send] {
        match self {
            Kind::Prompts | Kind::Dialogs => &[Send::Notify, Send::Thread],
            Kind::Notices => &[Send::Notify, Send::Off],
            _ => &[Send::Notify, Send::Thread, Send::Off],
        }
    }

    /// Whether a tab autopilot opened still gets it under "Landings and
    /// failures only".
    fn autopilot_keeps(self) -> bool {
        matches!(
            self,
            Kind::Prompts | Kind::Dialogs | Kind::Failures | Kind::Notices
        )
    }

    /// Whether it may still notify in quiet hours while "Prompts notify in
    /// quiet hours" is on.
    fn a_prompt(self) -> bool {
        matches!(self, Kind::Prompts | Kind::Dialogs)
    }
}

/// Where a message goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Send {
    /// The tab's thread and a line in the main DM.
    Notify,
    /// The tab's thread alone.
    Thread,
    /// Nowhere.
    Off,
}

impl Send {
    pub fn value(self) -> &'static str {
        match self {
            Send::Notify => "notify",
            Send::Thread => "thread",
            Send::Off => "off",
        }
    }

    pub fn of_value(value: &str) -> Option<Send> {
        [Send::Notify, Send::Thread, Send::Off]
            .into_iter()
            .find(|s| s.value() == value)
    }

    /// Its words on the view's switch.
    pub fn short(self) -> &'static str {
        match self {
            Send::Notify => "Notify",
            Send::Thread => "Thread",
            Send::Off => "Off",
        }
    }

    /// Its words in a menu and in what a change says.
    pub fn label(self) -> &'static str {
        match self {
            Send::Notify => "Notify me",
            Send::Thread => "Thread only",
            Send::Off => "Don't send",
        }
    }
}

pub const AUTOPILOT: &str = "relay_send_autopilot";
pub const QUIET: &str = "relay_quiet_hours";
pub const QUIET_FROM: &str = "relay_quiet_from";
pub const QUIET_TO: &str = "relay_quiet_to";
pub const QUIET_PROMPTS: &str = "relay_quiet_prompts";

/// A setting's value as text, or `None` when unset or of another type.
fn text(settings: &dyn Fn(&str) -> Option<Value>, key: &str) -> Option<String> {
    match settings(key)? {
        Value::String(s) => Some(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// What kind `kind`'s setting says, its default when unset or not one of
/// its choices (a prompt set to off by hand goes to its thread).
pub fn chosen(settings: &dyn Fn(&str) -> Option<Value>, kind: Kind) -> Send {
    match text(settings, &kind.key()).and_then(|v| Send::of_value(&v)) {
        Some(Send::Off) if !kind.choices().contains(&Send::Off) => Send::Thread,
        Some(Send::Thread) if !kind.choices().contains(&Send::Thread) => Send::Notify,
        Some(s) => s,
        None => kind.default(),
    }
}

/// Whether hour `hour` (0 to 23) is in quiet hours `from` to `to`, which
/// may run across midnight.
pub fn in_quiet(from: u32, to: u32, hour: u32) -> bool {
    match from.cmp(&to) {
        std::cmp::Ordering::Less => (from..to).contains(&hour),
        std::cmp::Ordering::Greater => hour >= from || hour < to,
        std::cmp::Ordering::Equal => false,
    }
}

/// Where a message of `kind` about a tab goes, at hour `hour` of the
/// computer's day: its setting, then autopilot's tabs, then quiet hours.
pub fn send_as(
    settings: &dyn Fn(&str) -> Option<Value>,
    kind: Kind,
    autopilot: bool,
    hour: u32,
) -> Send {
    let send = chosen(settings, kind);
    if autopilot
        && !kind.autopilot_keeps()
        && text(settings, AUTOPILOT).as_deref() == Some("landings")
    {
        return Send::Off;
    }
    let quiet = settings(QUIET) == Some(Value::Bool(true)) && kind != Kind::Notices;
    let hour_of = |key: &str, default: u32| {
        text(settings, key)
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|h| *h < 24)
            .unwrap_or(default)
    };
    if send == Send::Notify
        && quiet
        && in_quiet(hour_of(QUIET_FROM, 22), hour_of(QUIET_TO, 7), hour)
        && !(kind.a_prompt() && settings(QUIET_PROMPTS) != Some(Value::Bool(false)))
    {
        return Send::Thread;
    }
    send
}

/// `send_as` now, by the computer's clock, with the relay's settings.
pub fn send_now(cx: &super::Relay, kind: Kind, autopilot: bool) -> Send {
    send_as(
        &*cx.settings,
        kind,
        autopilot,
        util::local_now().hour() as u32,
    )
}

// ── What was sent ───────────────────────────────────────────────────────────

/// The sent file of the day `day` (`YYYY-MM-DD`).
pub fn sent_file(state_dir: &Path, day: &str) -> PathBuf {
    state_dir.join("sent").join(format!("{day}.tsv"))
}

/// One message sent: its kind, its tab's number and name (none for a
/// notice), whether the tab is autopilot's, and whether this is the line
/// in the main DM that notified you about it, recorded only once posted
/// (a message is one line; the notice that it reached you, another).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sent {
    pub kind: Kind,
    pub tab: Option<(i64, String)>,
    pub autopilot: bool,
    pub notified: bool,
}

/// Appends a message sent to today's file, and drops the files of days
/// over a week old as a new day's starts.
pub fn record(cx: &super::Relay, sent: &Sent) {
    let day = util::ymd(&util::local_now());
    let file = sent_file(&cx.paths.state_dir, &day);
    if !file.exists() {
        prune(&cx.paths.state_dir, &day);
    }
    let (index, name) = sent
        .tab
        .clone()
        .map_or((String::new(), String::new()), |(i, n)| (i.to_string(), n));
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    let line = format!(
        "{}\t{}\t{index}\t{}\t{}\t{}\n",
        util::now_ms(),
        sent.kind.name(),
        u8::from(sent.autopilot),
        u8::from(sent.notified),
        clean(&name)
    );
    util::append_quietly(file, line);
}

/// The sent files more than a week before `today`, removed.
fn prune(state_dir: &Path, today: &str) {
    let Ok(entries) = std::fs::read_dir(state_dir.join("sent")) else {
        return;
    };
    let Ok(today) = today.parse::<jiff::civil::Date>() else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let old = name
            .strip_suffix(".tsv")
            .and_then(|d| d.parse::<jiff::civil::Date>().ok())
            .is_some_and(|d| d < today.saturating_sub(jiff::Span::new().days(7)));
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// The messages a sent file holds, a line each.
pub fn read_sent(text: &str) -> Vec<Sent> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            let kind = Kind::of_name(f.get(1)?)?;
            let tab = f
                .get(2)
                .and_then(|i| i.parse::<i64>().ok())
                .map(|i| (i, f.get(5).unwrap_or(&"").to_string()));
            Some(Sent {
                kind,
                tab,
                autopilot: f.get(3) == Some(&"1"),
                notified: f.get(4) == Some(&"1"),
            })
        })
        .collect()
}

// ── Presets ─────────────────────────────────────────────────────────────────

/// The view's presets, and `switchboard slack-relay sends`'s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    NeedsMe,
    Everything,
    Quiet,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::NeedsMe, Preset::Everything, Preset::Quiet];

    /// Its name on the command line.
    pub fn slug(self) -> &'static str {
        match self {
            Preset::NeedsMe => "needs-me",
            Preset::Everything => "everything",
            Preset::Quiet => "quiet",
        }
    }

    pub fn of_slug(slug: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.slug() == slug)
    }

    pub fn label(self) -> &'static str {
        match self {
            Preset::NeedsMe => "Only what needs me",
            Preset::Everything => "Everything",
            Preset::Quiet => "Quiet",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Preset::NeedsMe => "what you must answer or fix notifies; the rest isn't sent",
            Preset::Everything => "every kind notifies",
            Preset::Quiet => "everything goes to its tab's thread; notices still notify",
        }
    }

    /// The settings it writes.
    pub fn values(self) -> Vec<(String, &'static str)> {
        let mut out: Vec<(String, &'static str)> = Kind::ALL
            .into_iter()
            .map(|k| {
                let send = match self {
                    Preset::Everything => Send::Notify,
                    Preset::Quiet if k == Kind::Notices => Send::Notify,
                    Preset::Quiet => Send::Thread,
                    Preset::NeedsMe => match k {
                        Kind::Prompts
                        | Kind::Dialogs
                        | Kind::Stuck
                        | Kind::Failures
                        | Kind::Notices => Send::Notify,
                        _ => Send::Off,
                    },
                };
                (k.key(), send.value())
            })
            .collect();
        out.push((
            AUTOPILOT.to_string(),
            if self == Preset::NeedsMe {
                "landings"
            } else {
                "same"
            },
        ));
        out
    }

    /// The preset the settings match, if any.
    pub fn matching(settings: &dyn Fn(&str) -> Option<Value>) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| {
            p.values().iter().all(|(key, value)| {
                let now = match Kind::ALL.into_iter().find(|k| k.key() == *key) {
                    Some(k) => chosen(settings, k).value().to_string(),
                    None => text(settings, key).unwrap_or_else(|| "same".into()),
                };
                now == *value
            })
        })
    }
}

/// The preset's settings written to the settings file; each one it changed,
/// with its label and its new value's words.
pub fn apply(preset: Preset) -> anyhow::Result<Vec<(String, String)>> {
    let current = crate::settings::read_settings();
    let mut changed = Vec::new();
    for (key, value) in preset.values() {
        let Some(option) = current.iter().find(|o| o.key == key) else {
            continue;
        };
        if option.value == Value::String(value.to_string()) {
            continue;
        }
        crate::settings::write_setting(&key, Value::String(value.to_string()))?;
        let words = option
            .choices
            .iter()
            .flatten()
            .find(|c| c.value == value)
            .map_or_else(|| value.to_string(), |c| c.label.clone());
        changed.push((option.label.clone(), words));
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn with(pairs: &[(&str, Value)]) -> impl Fn(&str) -> Option<Value> + use<> {
        let map: HashMap<String, Value> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    fn s(v: &str) -> Value {
        Value::String(v.into())
    }

    #[test]
    fn the_defaults_send_every_kind_where_the_relay_always_did() {
        let none = with(&[]);
        let at = |k| send_as(&none, k, false, 12);
        assert_eq!(at(Kind::Prompts), Send::Notify);
        assert_eq!(at(Kind::Dialogs), Send::Notify);
        assert_eq!(at(Kind::Finished), Send::Thread);
        assert_eq!(at(Kind::Stuck), Send::Notify);
        assert_eq!(at(Kind::Milestones), Send::Thread);
        assert_eq!(at(Kind::Failures), Send::Notify);
        assert_eq!(at(Kind::Context), Send::Thread);
        assert_eq!(at(Kind::Notices), Send::Notify);
    }

    #[test]
    fn the_defaults_and_choices_match_the_settings_file() {
        let schema = crate::settings::schema();
        for k in Kind::ALL {
            let o = schema
                .iter()
                .find(|o| o.key == k.key())
                .unwrap_or_else(|| panic!("{}", k.key()));
            assert_eq!(o.default, s(k.default().value()), "{}", k.key());
            let values: Vec<&str> = o
                .choices
                .iter()
                .flatten()
                .filter_map(|c| c.value.as_str())
                .collect();
            let ours: Vec<&str> = k.choices().iter().map(|c| c.value()).collect();
            assert_eq!(values, ours, "{}", k.key());
        }
        for key in [AUTOPILOT, QUIET, QUIET_FROM, QUIET_TO, QUIET_PROMPTS] {
            assert!(schema.iter().any(|o| o.key == key), "{key}");
        }
    }

    #[test]
    fn a_prompt_set_off_by_hand_still_reaches_its_thread() {
        let off = with(&[("relay_send_prompts", s("off"))]);
        assert_eq!(send_as(&off, Kind::Prompts, false, 12), Send::Thread);
        let thread = with(&[("relay_send_notices", s("thread"))]);
        assert_eq!(send_as(&thread, Kind::Notices, false, 12), Send::Notify);
    }

    #[test]
    fn landings_leaves_an_autopilot_tab_its_prompts_and_failures() {
        let landings = with(&[(AUTOPILOT, s("landings"))]);
        assert_eq!(send_as(&landings, Kind::Finished, true, 12), Send::Off);
        assert_eq!(send_as(&landings, Kind::Milestones, true, 12), Send::Off);
        assert_eq!(send_as(&landings, Kind::Stuck, true, 12), Send::Off);
        assert_eq!(send_as(&landings, Kind::Context, true, 12), Send::Off);
        assert_eq!(send_as(&landings, Kind::Failures, true, 12), Send::Notify);
        assert_eq!(send_as(&landings, Kind::Prompts, true, 12), Send::Notify);
        // Any other tab follows its kinds.
        assert_eq!(send_as(&landings, Kind::Finished, false, 12), Send::Thread);
    }

    #[test]
    fn quiet_hours_send_to_the_thread_but_keep_prompts() {
        let quiet = with(&[(QUIET, Value::Bool(true))]);
        // 22 to 7 runs across midnight.
        assert_eq!(send_as(&quiet, Kind::Stuck, false, 23), Send::Thread);
        assert_eq!(send_as(&quiet, Kind::Stuck, false, 3), Send::Thread);
        assert_eq!(send_as(&quiet, Kind::Stuck, false, 7), Send::Notify);
        assert_eq!(send_as(&quiet, Kind::Stuck, false, 21), Send::Notify);
        assert_eq!(send_as(&quiet, Kind::Prompts, false, 23), Send::Notify);
        // Within a day.
        let lunch = with(&[
            (QUIET, Value::Bool(true)),
            (QUIET_FROM, s("12")),
            (QUIET_TO, s("14")),
            (QUIET_PROMPTS, Value::Bool(false)),
        ]);
        assert_eq!(send_as(&lunch, Kind::Prompts, false, 13), Send::Thread);
        assert_eq!(send_as(&lunch, Kind::Prompts, false, 14), Send::Notify);
        // Notices aren't held.
        assert_eq!(send_as(&lunch, Kind::Notices, false, 13), Send::Notify);
        // Off, nothing changes.
        let off = with(&[(QUIET_FROM, s("0")), (QUIET_TO, s("23"))]);
        assert_eq!(send_as(&off, Kind::Stuck, false, 3), Send::Notify);
    }

    #[test]
    fn a_preset_is_lit_only_when_every_setting_matches_it() {
        assert_eq!(Preset::matching(&with(&[])), None);
        for p in Preset::ALL {
            let pairs: Vec<(&str, Value)> = p
                .values()
                .iter()
                .map(|(k, v)| (&*k.clone().leak(), s(v)))
                .collect();
            assert_eq!(Preset::matching(&with(&pairs)), Some(p));
        }
        let needs = Preset::NeedsMe.values();
        assert!(needs.contains(&("relay_send_finished".into(), "off")));
        assert!(needs.contains(&("relay_send_failures".into(), "notify")));
        assert!(needs.contains(&(AUTOPILOT.into(), "landings")));
    }

    #[test]
    fn the_sent_file_reads_back_what_was_recorded() {
        let text = "1\tprompts\t5\t1\t1\tFix it\n2\tnotices\t\t0\t1\t\nbad line\n3\tfinished\t3\t0\t0\tx\ty\n";
        let sent = read_sent(text);
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[0].tab, Some((5, "Fix it".into())));
        assert!(sent[0].autopilot && sent[0].notified);
        assert_eq!(sent[1].kind, Kind::Notices);
        assert_eq!(sent[1].tab, None);
        assert!(!sent[2].notified);
    }
}
