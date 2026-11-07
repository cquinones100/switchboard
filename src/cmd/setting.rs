//! `switchboard setting <key> [<value> | --reset]`: one of the workspace's
//! settings, as Settings reads it (src/settings.rs, your `settings.json` over
//! the defaults): `on` or `off`, or a choice's value. For a skill that
//! follows a setting, as /review-loop skips its security review when
//! `review_security` is off, where the tmux version's skill parsed the
//! settings file with `node -e` to read it the same way.
//!
//! With a value it changes the setting, as `tmux/settings.sh set` did: `on`
//! or `off` (`true`, `false`) for a switch, one of its choices' values for
//! the rest, anything else refused; `--reset` gives it its default back.
//! The write is `settings::write_setting`'s, the one Settings makes: never
//! over a file that can't be read as a JSON object, the version before kept
//! as `settings.json.bak`, a value equal to its default left out. A running
//! workspace takes it at once, as it reads the file again on any change.
//! Changing `claude_plugin` moves Switchboard into its plugin or back into
//! `~/.claude` after, as Settings does (`switchboard setup --move`).

use std::process::ExitCode;

use serde_json::Value;

use crate::{settings, util};

const USAGE: &str = "Usage: switchboard setting <key> [<value> | --reset]";

/// `switchboard setting`.
pub fn main(args: Vec<String>) -> ExitCode {
    let reset = args.iter().any(|a| a == "--reset");
    let words: Vec<&String> = args.iter().filter(|a| *a != "--reset").collect();
    let (key, value) = match words.as_slice() {
        [key] if !key.starts_with('-') => (key.as_str(), None),
        [key, value] if !key.starts_with('-') && !reset => (key.as_str(), Some(value.as_str())),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let Some(option) = settings::schema().into_iter().find(|o| o.key == key) else {
        eprintln!("switchboard setting: no setting named {key}");
        return ExitCode::from(2);
    };
    let new = match (value, reset) {
        (None, false) => {
            let now = settings::setting(key).unwrap_or(option.default);
            println!("{}", words_of(&now));
            return ExitCode::SUCCESS;
        }
        (None, true) => option.default.clone(),
        (Some(given), _) => match value_of(&option, given) {
            Some(value) => value,
            None => {
                eprintln!(
                    "switchboard setting: not a value for {key}: {given} (it takes {})",
                    takes(&option)
                );
                return ExitCode::from(2);
            }
        },
    };
    match settings::write_setting(key, new.clone()) {
        Ok(()) if key == crate::setup::way::SETTING => {
            println!("{}", words_of(&new));
            crate::setup::main(vec!["--move".to_string()])
        }
        Ok(()) => {
            println!("{}", words_of(&new));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("switchboard setting: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The value `given` stands for: on/off (or true/false) for a switch, a
/// choice's value as written for the rest.
fn value_of(option: &settings::Setting, given: &str) -> Option<Value> {
    match &option.choices {
        None => match given {
            "on" | "true" => Some(Value::Bool(true)),
            "off" | "false" => Some(Value::Bool(false)),
            _ => None,
        },
        Some(choices) => choices
            .iter()
            .find(|c| util::js_string(&c.value) == given)
            .map(|c| c.value.clone()),
    }
}

/// What an option takes, for the refusal.
fn takes(option: &settings::Setting) -> String {
    match &option.choices {
        None => "on or off".into(),
        Some(choices) => choices
            .iter()
            .map(|c| util::js_string(&c.value))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// A value as printed: on/off for a switch, else the choice as written.
fn words_of(value: &Value) -> String {
    match value {
        Value::Bool(true) => "on".into(),
        Value::Bool(false) => "off".into(),
        other => util::js_string(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_read_as_on_and_off() {
        assert_eq!(words_of(&Value::Bool(false)), "off");
        assert_eq!(words_of(&Value::Bool(true)), "on");
        assert_eq!(words_of(&Value::from("10m")), "10m");
    }

    #[test]
    fn only_a_value_the_option_takes_is_taken() {
        let schema = settings::schema();
        let switch = schema.iter().find(|o| o.choices.is_none()).unwrap();
        assert_eq!(value_of(switch, "off"), Some(Value::Bool(false)));
        assert_eq!(value_of(switch, "true"), Some(Value::Bool(true)));
        assert_eq!(value_of(switch, "maybe"), None);
        let chosen = schema.iter().find(|o| o.choices.is_some()).unwrap();
        let first = &chosen.choices.as_ref().unwrap()[0].value;
        assert_eq!(
            value_of(chosen, &util::js_string(first)),
            Some(first.clone())
        );
        assert_eq!(value_of(chosen, "no-such-choice"), None);
    }
}
