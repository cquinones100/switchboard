//! What a mission control view says when what it reads from isn't set up
//! yet: what is missing and how to set it up, so a view that is on is always
//! there to go to and never just empty or a bare error, as
//! lib/sidebar/setup-help.js said it. Its words for gh (not installed, not
//! signed in) are gone with gh: GitHub is asked over its API with a token,
//! and the integrations' own errors say where one is looked for.
//!
//! A failed look is set on its view here: a missing or refused token is a
//! setup, drawn whole in place of the items; anything else (the network, a
//! rate limit, GitHub's own error) is an error, shown above the items the
//! view kept from its last answer.

use crate::items::ViewData;
use crate::net::{ErrorKind, ServiceError};
use crate::util;

pub const GITHUB: &str = "GitHub isn't set up: this needs a GitHub token. Set GH_TOKEN, or sign in with gh auth login if you use GitHub's CLI, or make a token on GitHub (Settings, Developer settings, Personal access tokens) and store it: run switchboard platform secret-set github-token in a terminal and paste the token. Mission control picks it up within two minutes.";

pub const LINEAR: &str = "Linear isn't set up: this needs a Linear API key. In Linear, open Settings, Security & access, and make a personal API key, then store it: run switchboard platform secret-set linear-api-key in a terminal and paste the key. Mission control picks it up within two minutes.";

pub const NO_REPOS: &str = "None of your tabs is in a GitHub repository yet: open a tab in one (a clone with an origin on github.com) and its pull requests show here.";

/// A view that answered: what it had to say, with nothing wrong.
pub fn answered(view: &mut ViewData) {
    view.setup = None;
    view.error = None;
    view.at = Some(util::now_ms());
}

/// A view with nothing to look at until the tabs are in a GitHub repository.
pub fn no_repos(view: &mut ViewData) {
    view.items.clear();
    view.setup = Some(NO_REPOS.to_string());
    view.error = None;
    view.at = Some(util::now_ms());
}

/// A view whose look failed, `what` saying what was asked ("your pull
/// requests' checks"): its setup words when the service has no token or
/// refused it, else the error above what it kept.
pub fn failed(view: &mut ViewData, err: &anyhow::Error, what: &str) {
    view.at = Some(util::now_ms());
    let service = err.downcast_ref::<ServiceError>();
    match service.map(|e| (&e.kind, e.service.as_str())) {
        Some((ErrorKind::NoCredentials, "GitHub")) => {
            view.setup = Some(GITHUB.to_string());
            view.error = None;
        }
        Some((ErrorKind::NoCredentials, "Linear")) => {
            view.setup = Some(LINEAR.to_string());
            view.error = None;
        }
        Some((ErrorKind::Unauthorized | ErrorKind::Off | ErrorKind::NoCredentials, _)) => {
            view.setup = Some(err.to_string());
            view.error = None;
        }
        _ => {
            view.setup = None;
            let why = err.to_string();
            let first = why.lines().next().unwrap_or("no answer");
            view.error = Some(format!("Couldn't ask about {what}: {first}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::Item;

    #[test]
    fn a_missing_token_is_a_setup_and_anything_else_an_error_above_what_was_kept() {
        let mut view = ViewData {
            items: vec![Item::new("1", "#1 kept")],
            ..ViewData::default()
        };
        let down: anyhow::Error = ServiceError::new(
            ErrorKind::Unreachable,
            "GitHub",
            "Couldn't reach GitHub: connection refused",
        )
        .into();
        failed(&mut view, &down, "your pull requests");
        assert_eq!(view.items.len(), 1, "the items are kept");
        assert_eq!(
            view.error.as_deref(),
            Some(
                "Couldn't ask about your pull requests: Couldn't reach GitHub: connection refused"
            )
        );
        let none: anyhow::Error =
            ServiceError::new(ErrorKind::NoCredentials, "Linear", "No Linear API key").into();
        failed(&mut view, &none, "your tickets");
        assert_eq!(view.setup.as_deref(), Some(LINEAR));
        assert_eq!(view.error, None);
        answered(&mut view);
        assert_eq!((view.setup.as_deref(), view.error.as_deref()), (None, None));
        no_repos(&mut view);
        assert!(view.items.is_empty());
        assert_eq!(view.setup.as_deref(), Some(NO_REPOS));
    }
}
