//! `switchboard setting <key>`: one of the workspace's settings, as
//! Settings reads it (src/settings.rs, your `settings.json` over the
//! defaults): `on` or `off`, or a choice's value. For a skill that follows a
//! setting, as /review-loop skips its security review when
//! `review_security` is off, where the tmux version's skill parsed the
//! settings file with `node -e` to read it the same way.

use std::process::ExitCode;

use serde_json::Value;

use crate::{settings, util};

/// `switchboard setting`.
pub fn main(args: Vec<String>) -> ExitCode {
    let Some(key) = args.first().filter(|k| !k.starts_with('-')) else {
        eprintln!("Usage: switchboard setting <key>");
        return ExitCode::from(2);
    };
    match settings::setting(key) {
        Some(value) => {
            println!("{}", words(&value));
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("switchboard setting: no setting named {key}");
            ExitCode::from(2)
        }
    }
}

/// A value as printed: on/off for a switch, else the choice as written.
fn words(value: &Value) -> String {
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
        assert_eq!(words(&Value::Bool(false)), "off");
        assert_eq!(words(&Value::Bool(true)), "on");
        assert_eq!(words(&Value::from("10m")), "10m");
    }
}
