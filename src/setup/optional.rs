//! What the guided setup says of the optional parts, each for the features
//! it names: setup.sh's "Optional, each for the keys it names". Nothing here
//! changes anything; each line says whether the part is there and, when it
//! isn't, how to put it there. Mission control's views say the same of the
//! services they read when they are opened; these are said in one place,
//! with the programs the skills' scripts need beside them, so a fresh
//! install learns of each before a feature fails for want of it.

use serde_json::Value;

use crate::integrations::{github, linear};
use crate::{day, paths, platform, relay, settings, util, views};

/// One optional part: whether it is there, the line said of it, and its
/// name, for the list at the end.
pub struct Optional {
    pub there: bool,
    pub what: String,
    pub name: String,
}

/// A part's line: `yes` when it is there, `no` (what it is for and how to
/// set it up) when it isn't; its name is `yes` up to its parenthesis.
fn line(there: bool, yes: &str, no: &str) -> Optional {
    Optional {
        there,
        what: if there { yes } else { no }.to_string(),
        name: yes.split(" (").next().unwrap_or(yes).to_string(),
    }
}

fn has(program: &str) -> bool {
    util::which(program).is_some()
}

/// Every optional part, in the order said.
pub fn all() -> Vec<Optional> {
    let mut all = Vec::new();
    if settings::is_on("github") {
        all.push(line(
            util::block_on(github::token()).is_ok(),
            "GitHub (M-v, M-d's base, the PRs and CI views, PR numbers in tab names)",
            "GitHub, for M-v, M-d's base, the PRs and CI views and PR numbers in tab names: install GitHub's CLI (gh) and run gh auth login",
        ));
    }
    if settings::is_on("linear") {
        all.push(line(
            util::block_on(linear::api_key()).is_ok(),
            "Linear (M-i, the Tickets and Inbox views, ticket titles in tab names)",
            "Linear, for M-i, the Tickets and Inbox views and ticket titles in tab names: a personal API key from Linear's Settings, Security & access, stored with switchboard platform secret-set linear-api-key",
        ));
        let team = util::read_json(paths::config_dir().join("linear.json"))
            .and_then(|v| {
                v.get("linearTeam")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .is_some_and(|t| !t.trim().is_empty());
        all.push(line(
            team,
            "the Linear team /shape files tickets in",
            "the Linear team /shape files tickets in: \"linearTeam\": \"<team key>\" in ~/.config/switchboard/linear.json (or Settings, linear.json)",
        ));
    }
    all.push(line(
        platform::secret(day::calendar::SECRET).is_some(),
        "your calendar's address (the Calendar view)",
        "your calendar's address, for the Calendar view without the Calendar routine: switchboard platform secret-set google-calendar-ical, then paste Google Calendar's secret address in iCal format",
    ));
    let relay = relay::slack::token(relay::slack::BOT_TOKEN_SERVICE).is_some()
        && relay::slack::token(relay::slack::APP_TOKEN_SERVICE).is_some()
        && util::read_json(paths::config_dir().join("slack-relay.json"))
            .is_some_and(|v| v.get("owner").and_then(Value::as_str).is_some());
    all.push(line(
        relay,
        "the Slack relay's tokens and owner (M-S)",
        "the Slack relay, for reaching your tabs from Slack (M-S): its app's tokens (switchboard platform secret-set slack-relay-bot-token and slack-relay-app-token) and your member ID in ~/.config/switchboard/slack-relay.json, as the guide's \"The Slack relay\" says",
    ));
    let chrome = views::image::render::find_chrome().is_some();
    all.push(line(
        chrome,
        "Chrome (the PR and ticket views as images, /walkthrough's videos)",
        "Chrome, for the PR and ticket views as images (M-p) and /walkthrough, /deploy-test and /preview-test's videos",
    ));
    let node = has("node") && has("npm");
    all.push(line(
        has("ffmpeg") && node,
        "ffmpeg and Node (/walkthrough, /deploy-test, /preview-test)",
        "ffmpeg and Node, for /walkthrough, /deploy-test and /preview-test, which record a browser and make the video an mp4",
    ));
    all.push(line(
        has("delta") && node && chrome,
        "git-delta, Node and Chrome (/diff-screenshots)",
        "git-delta, Node and Chrome, for /diff-screenshots, which draws a diff as images",
    ));
    all.push(line(
        has("codex"),
        "Codex (/review-loop's reviewer)",
        "Codex, /review-loop's reviewer (Claude reviews without it): npm install -g @openai/codex, then codex login",
    ));
    all
}
