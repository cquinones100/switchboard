//! What acts on a whole view, in its `m` menu: the views whose items each
//! have a menu (Tabs and the Work map, Slack, the Slack relay, PRs, CI,
//! Landing, Tickets) offer here what applies to all they show, which no
//! one item's menu could hold (every Slack message shown dismissed, every
//! PR read again, main's failed jobs run again). A plain function of the
//! view and of what the server already knows (`Facts`), so the menu is the
//! same tested without a server; `serve`'s `view_menu` adds what follows
//! these (what the view shows, several views at once, its settings) and
//! does what is picked. An action that uses GitHub or Linear is left out
//! while that service's setting is off, as the palette leaves out what
//! carries its tag; one that changes anything outside the computer asks
//! first when it is done, as its item's version does. New with this menu;
//! no tmux-version file is behind it.

use std::path::PathBuf;

use serde_json::Value;

use super::choices::Do;
use super::slack_board;
use super::views::ViewId;
use crate::items::{Item, Target};
use crate::mux::TabId;
use crate::mux::keys::Action;
use crate::relay::sending::Preset;

/// What an option of a view's `m` menu does over the whole view.
#[derive(Clone, Debug, PartialEq)]
pub enum Whole {
    /// What an item's menu could do too, done as it is there.
    Do(Do),
    /// A lookup of the sources asked now, by its view (`sources::VIEWS`).
    Refresh(&'static str),
    /// Slack looked at now: Claude Code asked about its connector while it
    /// is down, else the server's own look.
    CheckSlack { down: bool },
    /// Each conversation shown marked read in Slack up to its newest
    /// message shown: `(channel, ts)`.
    SlackMarkRead(Vec<(String, String)>),
    /// The failed jobs of each failing default branch's run started again,
    /// asked first: `(owner/name, run id)`.
    RerunMain(Vec<(String, String)>),
    /// A ticket typed in, shaped by `/shape` in a tab of the repository at
    /// `dir`, filed in its ticket source (`None` while it isn't set).
    Shape {
        dir: PathBuf,
        source: Option<String>,
    },
}

/// What the menu needs to know beyond the view.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub github: bool,
    pub linear: bool,
    /// Settings' `slack_background`: the server looks at Slack itself.
    pub slack_background: bool,
    /// Whether Claude Code's Slack connector is down.
    pub slack_down: bool,
    /// Settings' `slack_mark_read` on, with a Slack sign-in stored.
    pub slack_mark_read: bool,
    /// The tabs under Merged.
    pub merged: Vec<TabId>,
    /// The front pane's repository: its main clone, its `owner/name` on
    /// GitHub, and its ticket source (repos.json's `tickets`).
    pub dir: Option<PathBuf>,
    pub repo: Option<String>,
    pub tickets: Option<String>,
}

/// The view's actions over all it shows, in the order the menu lists them.
/// `id` is `None` for the Tabs view; `shown` is what the view shows now
/// (the Slack board's section), `items` all it holds.
pub fn of(id: Option<&ViewId>, items: &[Item], shown: &[Item], f: &Facts) -> Vec<(String, Whole)> {
    let mut out: Vec<(String, Whole)> = Vec::new();
    let mut add = |label: &str, act: Whole| out.push((label.to_string(), act));
    match id {
        None | Some(ViewId::WorkMap) => {
            if !f.merged.is_empty() {
                add(
                    "Close every merged tab…",
                    Whole::Do(Do::CloseMerged(f.merged.clone())),
                );
            }
            add(
                "Go to the next tab that needs you",
                Whole::Do(Do::Press(Action::NextNeedsYou)),
            );
        }
        Some(ViewId::Slack) => {
            let messages: Vec<Item> = shown
                .iter()
                .filter(|i| i.data.get("message").is_some())
                .flat_map(slack_board::members)
                .collect();
            if !messages.is_empty() {
                add(
                    "Dismiss every message shown",
                    Whole::Do(Do::SlackDismiss(messages.clone())),
                );
                let newest = newest_by_channel(&messages);
                if f.slack_mark_read && !newest.is_empty() {
                    add(
                        "Mark every message shown read in Slack",
                        Whole::SlackMarkRead(newest),
                    );
                }
            }
            if f.slack_background {
                add(
                    "Check Slack again",
                    Whole::CheckSlack { down: f.slack_down },
                );
            }
        }
        Some(ViewId::Relay) => {
            for (label, preset) in [
                ("Send everything", Preset::Everything),
                ("Send only what needs me", Preset::NeedsMe),
                ("Quiet", Preset::Quiet),
            ] {
                add(label, Whole::Do(Do::RelayPreset(preset)));
            }
        }
        Some(ViewId::Prs) if f.github => {
            add(
                "Read every PR again now",
                Whole::Refresh(crate::sources::PRS),
            );
            add(
                "Open the PRs waiting on your review on GitHub",
                Whole::Do(Do::Open("https://github.com/pulls/review-requested".into())),
            );
        }
        Some(ViewId::Ci) if f.github => {
            add("Read CI again now", Whole::Refresh(crate::sources::CI));
            let runs = main_runs(items);
            if !runs.is_empty() {
                add("Re-run main's failed jobs…", Whole::RerunMain(runs));
            }
        }
        Some(ViewId::Landing) if f.github => {
            let mut queues: Vec<&str> = items
                .iter()
                .filter_map(|i| i.data.pointer("/queue/queueUrl")?.as_str())
                .filter(|u| !u.is_empty())
                .collect();
            queues.sort_unstable();
            queues.dedup();
            for url in &queues {
                let label = match (queues.len(), repo_of_github(url)) {
                    (1, _) | (_, None) => "Open the merge queue on GitHub".to_string(),
                    (_, Some(repo)) => format!("Open {repo}'s merge queue on GitHub"),
                };
                add(&label, Whole::Do(Do::Open(url.to_string())));
            }
        }
        Some(ViewId::Tickets) => {
            if f.linear {
                add(
                    "Read tickets again now",
                    Whole::Refresh(crate::sources::TICKETS),
                );
            }
            let source = f.tickets.as_deref();
            let service_on = match source {
                Some("github") => f.github,
                Some("linear") => f.linear,
                _ => true,
            };
            if let Some(dir) = f.dir.as_ref().filter(|_| service_on) {
                add(
                    &shape_label(source),
                    Whole::Shape {
                        dir: dir.clone(),
                        source: source.map(str::to_string),
                    },
                );
            }
            match source {
                Some("linear") if f.linear => {
                    if let Some(url) = linear_inbox(items) {
                        add("Open your Linear inbox", Whole::Do(Do::Open(url)));
                    }
                }
                Some("github") if f.github => {
                    if let Some(repo) = &f.repo {
                        add(
                            &format!("Open {repo}'s issues on GitHub"),
                            Whole::Do(Do::Open(format!("https://github.com/{repo}/issues"))),
                        );
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
    out
}

/// "Shape a new ticket…" naming where it is filed.
pub fn shape_label(source: Option<&str>) -> String {
    match source {
        Some("github") => "Shape a new ticket as a GitHub issue…".into(),
        Some("linear") => "Shape a new ticket in Linear…".into(),
        Some("notes") => "Shape a new ticket as a note's item…".into(),
        _ => "Shape a new ticket…".into(),
    }
}

/// Each channel's newest message among `messages`, in the order the
/// channels first appear.
fn newest_by_channel(messages: &[Item]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for item in messages {
        let Some(m) = item.data.get("message") else {
            continue;
        };
        let get = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let (channel, ts) = (get("channel"), get("ts"));
        if channel.is_empty() || ts.parse::<f64>().is_err() {
            continue;
        }
        match out.iter_mut().find(|(c, _)| *c == channel) {
            Some((_, kept)) => {
                if ts.parse::<f64>().unwrap_or(0.0) > kept.parse::<f64>().unwrap_or(0.0) {
                    *kept = ts;
                }
            }
            None => out.push((channel, ts)),
        }
    }
    out
}

/// The runs of the failing default branches the CI view lists
/// (src/sources/mainci.rs), `(owner/name, run id)`, each once.
fn main_runs(items: &[Item]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for item in items.iter().filter(|i| i.data.get("mainCi").is_some()) {
        let repo = item.data["mainCi"]["repo"].as_str().unwrap_or("");
        let runs = item.data["ci"]["checks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["run"].as_str());
        for run in runs {
            let pair = (repo.to_string(), run.to_string());
            if !repo.is_empty() && !out.contains(&pair) {
                out.push(pair);
            }
        }
    }
    out
}

/// `owner/name` of a github.com address.
fn repo_of_github(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/")?;
    let mut parts = rest.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    (!owner.is_empty() && !name.is_empty()).then(|| format!("{owner}/{name}"))
}

/// Your Linear inbox, in the workspace the Tickets view's tickets link to.
fn linear_inbox(items: &[Item]) -> Option<String> {
    items.iter().find_map(|i| {
        let Target::Url(url) = &i.target else {
            return None;
        };
        let workspace = url.strip_prefix("https://linear.app/")?.split('/').next()?;
        (!workspace.is_empty()).then(|| format!("https://linear.app/{workspace}/inbox"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn labels(list: &[(String, Whole)]) -> Vec<&str> {
        list.iter().map(|(l, _)| l.as_str()).collect()
    }

    fn on() -> Facts {
        Facts {
            github: true,
            linear: true,
            slack_background: true,
            slack_mark_read: true,
            dir: Some(PathBuf::from("/code/app")),
            repo: Some("acme/app".into()),
            tickets: Some("linear".into()),
            ..Facts::default()
        }
    }

    fn message(id: &str, channel: &str, ts: &str) -> Item {
        Item::new(id, id).data(json!({ "message": { "channel": channel, "ts": ts } }))
    }

    fn main_ci(repo: &str, run: &str) -> Item {
        Item::new(format!("main-ci:{repo}"), "main failing").data(json!({
            "ci": { "checks": [{ "run": run }] },
            "mainCi": { "repo": repo },
        }))
    }

    fn ticket() -> Item {
        Item::new("t", "AB-1").target(Target::Url("https://linear.app/acme/issue/AB-1/x".into()))
    }

    #[test]
    fn each_view_lists_its_actions_over_the_whole_view() {
        let mut f = on();
        f.merged = vec![TabId(3)];
        assert_eq!(
            labels(&of(None, &[], &[], &f)),
            [
                "Close every merged tab…",
                "Go to the next tab that needs you"
            ]
        );
        let shown = [message("a", "C1", "1.0")];
        assert_eq!(
            labels(&of(Some(&ViewId::Slack), &shown, &shown, &f)),
            [
                "Dismiss every message shown",
                "Mark every message shown read in Slack",
                "Check Slack again"
            ]
        );
        assert_eq!(
            labels(&of(Some(&ViewId::Relay), &[], &[], &f)),
            ["Send everything", "Send only what needs me", "Quiet"]
        );
        assert_eq!(
            labels(&of(Some(&ViewId::Prs), &[], &[], &f)),
            [
                "Read every PR again now",
                "Open the PRs waiting on your review on GitHub"
            ]
        );
        let ci = [main_ci("acme/app", "77")];
        assert_eq!(
            labels(&of(Some(&ViewId::Ci), &ci, &ci, &f)),
            ["Read CI again now", "Re-run main's failed jobs…"]
        );
        let landing = [Item::new("q", "q")
            .data(json!({ "queue": { "queueUrl": "https://github.com/acme/app/queue/main" } }))];
        assert_eq!(
            labels(&of(Some(&ViewId::Landing), &landing, &landing, &f)),
            ["Open the merge queue on GitHub"]
        );
        let tickets = [ticket()];
        assert_eq!(
            labels(&of(Some(&ViewId::Tickets), &tickets, &tickets, &f)),
            [
                "Read tickets again now",
                "Shape a new ticket in Linear…",
                "Open your Linear inbox"
            ]
        );
    }

    #[test]
    fn a_service_turned_off_takes_its_actions_out() {
        let f = Facts {
            github: false,
            linear: false,
            ..on()
        };
        for id in [ViewId::Prs, ViewId::Ci, ViewId::Landing, ViewId::Tickets] {
            let items = [main_ci("acme/app", "77"), ticket()];
            assert!(
                of(Some(&id), &items, &items, &f).is_empty(),
                "{id:?}: {:?}",
                labels(&of(Some(&id), &items, &items, &f))
            );
        }
        // A repository whose tickets are notes still shapes with both off.
        let notes = Facts {
            tickets: Some("notes".into()),
            ..f
        };
        assert_eq!(
            labels(&of(Some(&ViewId::Tickets), &[], &[], &notes)),
            ["Shape a new ticket as a note's item…"]
        );
    }

    #[test]
    fn slacks_actions_take_every_message_shown_and_check_while_up_or_down() {
        let mut f = on();
        let row = Item::new("row", "thread").data(json!({
            "message": { "channel": "C1", "ts": "2.0" },
            "members": [
                { "id": "a", "title": "a", "data": { "message": { "channel": "C1", "ts": "1.0" } } },
                { "id": "b", "title": "b", "data": { "message": { "channel": "C1", "ts": "3.0" } } },
            ],
        }));
        let other = message("c", "C2", "5.0");
        let shown = [row.clone(), other.clone()];
        let list = of(Some(&ViewId::Slack), &shown, &shown, &f);
        let Whole::Do(Do::SlackDismiss(dismissed)) = &list[0].1 else {
            panic!("{list:?}");
        };
        let ids: Vec<&str> = dismissed.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(
            list[1].1,
            Whole::SlackMarkRead(vec![
                ("C1".into(), "3.0".into()),
                ("C2".into(), "5.0".into())
            ])
        );
        assert_eq!(list[2].1, Whole::CheckSlack { down: false });
        f.slack_down = true;
        f.slack_mark_read = false;
        let list = of(Some(&ViewId::Slack), &[], &[], &f);
        assert_eq!(labels(&list), ["Check Slack again"]);
        assert_eq!(list[0].1, Whole::CheckSlack { down: true });
    }

    #[test]
    fn re_running_main_names_each_failing_run_once() {
        let items = [
            main_ci("acme/app", "77"),
            main_ci("acme/app", "77"),
            main_ci("acme/lib", "9"),
            Item::new("pr", "a PR").data(json!({ "ci": { "checks": [{ "run": "5" }] } })),
        ];
        let list = of(Some(&ViewId::Ci), &items, &items, &on());
        assert_eq!(
            list[1].1,
            Whole::RerunMain(vec![
                ("acme/app".into(), "77".into()),
                ("acme/lib".into(), "9".into())
            ])
        );
        // Nothing of main's failing: nothing to run again.
        assert_eq!(
            labels(&of(Some(&ViewId::Ci), &items[3..], &[], &on())),
            ["Read CI again now"]
        );
    }

    #[test]
    fn shaping_a_ticket_names_the_repositorys_own_ticket_source() {
        for (source, label, inbox) in [
            (
                Some("github"),
                "Shape a new ticket as a GitHub issue…",
                Some("Open acme/app's issues on GitHub"),
            ),
            (
                Some("linear"),
                "Shape a new ticket in Linear…",
                Some("Open your Linear inbox"),
            ),
            (Some("notes"), "Shape a new ticket as a note's item…", None),
            (None, "Shape a new ticket…", None),
        ] {
            let f = Facts {
                tickets: source.map(str::to_string),
                ..on()
            };
            let items = [ticket()];
            let list = of(Some(&ViewId::Tickets), &items, &items, &f);
            assert_eq!(list[1].0, label);
            assert_eq!(
                list[1].1,
                Whole::Shape {
                    dir: PathBuf::from("/code/app"),
                    source: source.map(str::to_string)
                }
            );
            assert_eq!(list.get(2).map(|(l, _)| l.as_str()), inbox);
        }
        // With no front pane in a repository, nothing to shape in.
        let f = Facts { dir: None, ..on() };
        assert_eq!(
            labels(&of(Some(&ViewId::Tickets), &[], &[], &f)),
            ["Read tickets again now"]
        );
    }
}
