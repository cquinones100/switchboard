//! `switchboard` alone: the workspace, started or attached to, and
//! `switchboard -s <name>` for a session by name. The client does the work
//! (`mux::client`); this reads the arguments and says plainly what went
//! wrong, since it is the first thing someone runs.

use std::process::ExitCode;

use crate::mux::client;

pub fn main(args: Vec<String>) -> ExitCode {
    let session = match session_of(&args) {
        Ok(session) => session,
        Err(e) => {
            eprintln!("switchboard: {e}");
            return ExitCode::from(2);
        }
    };
    // The client's errors are whole sentences that name Switchboard where
    // it matters ("switchboard needs a terminal", "already inside
    // Switchboard"), so they are printed as they are.
    match client::attach(session) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

/// The session asked for: none for the default, or the name after `-s`.
fn session_of(args: &[String]) -> Result<Option<String>, String> {
    match args {
        [] => Ok(None),
        [flag, name] if flag == "-s" && !name.is_empty() => Ok(Some(name.clone())),
        [flag] | [flag, _] if flag == "-s" => Err("-s needs a session's name".into()),
        _ => Err(format!(
            "didn't expect {} (switchboard help lists the commands)",
            args.join(" ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn no_arguments_is_the_default_session() {
        assert_eq!(session_of(&[]), Ok(None));
    }

    #[test]
    fn dash_s_names_the_session() {
        assert_eq!(session_of(&args(&["-s", "work"])), Ok(Some("work".into())));
    }

    #[test]
    fn dash_s_needs_a_name() {
        assert!(session_of(&args(&["-s"])).is_err());
        assert!(session_of(&args(&["-s", ""])).is_err());
    }

    #[test]
    fn anything_else_is_refused() {
        assert!(session_of(&args(&["-s", "a", "b"])).is_err());
        assert!(session_of(&args(&["-x"])).is_err());
    }
}
