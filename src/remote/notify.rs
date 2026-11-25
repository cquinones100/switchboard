//! The one notification policy: what reaches you from a remote front-end,
//! and how loudly. The Slack relay asks it of each message it says by
//! itself about a tab, and the phone app of each push, through `decide`,
//! each front-end's own settings its inputs.
//!
//! The relay's kinds (`Kind`) each have a setting in the "Slack relay"
//! category (`relay_send_<kind>`): Notify (the tab's thread, with a line
//! there mentioning you, which is what Slack notifies you for), Thread (the
//! thread alone) or Off (nothing posted, though the message still counts
//! as reported, so turning the kind back on doesn't post a backlog).
//! `relay_send_autopilot` keeps a tab autopilot opened to its prompts,
//! dialogs and failures, and quiet hours turn Notify into Thread by the
//! computer's clock. The phone pushes a tab that starts waiting on you (a
//! prompt) and a reminder come due (a notice) while Settings' `phone` is
//! on, and nothing else. The settings are read as each message goes out,
//! so a change takes effect with the next one. Moved from the relay's
//! src/relay/sending.rs, where the relay's sent file and presets stay.

use serde_json::Value;

/// The front-end a message goes out through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Front {
    Slack,
    Phone,
}

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
    /// The tab's thread, with a line there mentioning you.
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
pub fn text(settings: &dyn Fn(&str) -> Option<Value>, key: &str) -> Option<String> {
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

/// Where a message of `kind` about a tab goes through `front`, at hour
/// `hour` of the computer's day, by `settings`.
pub fn decide(
    front: Front,
    settings: &dyn Fn(&str) -> Option<Value>,
    kind: Kind,
    autopilot: bool,
    hour: u32,
) -> Send {
    match front {
        Front::Slack => send_as(settings, kind, autopilot, hour),
        Front::Phone
            if settings("phone") == Some(Value::Bool(true))
                && matches!(kind, Kind::Prompts | Kind::Notices) =>
        {
            Send::Notify
        }
        Front::Phone => Send::Off,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phone_pushes_prompts_and_reminders_while_it_is_on() {
        let on = |k: &str| (k == "phone").then_some(Value::Bool(true));
        let off = |_: &str| None;
        assert_eq!(
            decide(Front::Phone, &on, Kind::Prompts, false, 3),
            Send::Notify
        );
        assert_eq!(
            decide(Front::Phone, &on, Kind::Notices, true, 23),
            Send::Notify
        );
        assert_eq!(
            decide(Front::Phone, &on, Kind::Finished, false, 12),
            Send::Off
        );
        assert_eq!(
            decide(Front::Phone, &off, Kind::Prompts, false, 12),
            Send::Off
        );
        // The relay's settings and quiet hours are the relay's alone.
        let quiet = |k: &str| match k {
            "phone" | QUIET => Some(Value::Bool(true)),
            QUIET_PROMPTS => Some(Value::Bool(false)),
            _ => None,
        };
        assert_eq!(
            decide(Front::Phone, &quiet, Kind::Prompts, false, 23),
            Send::Notify
        );
        assert_eq!(
            decide(Front::Slack, &quiet, Kind::Prompts, false, 23),
            Send::Thread
        );
    }
}
