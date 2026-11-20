//! This program answering the diff for the workspace: the plugin API's
//! `Host`, for a plugin loaded in process. Whatever the diff
//! (`switchboard-diff`) needs that isn't its own, it asks here, so each
//! rule the code keeps is kept in one place: a setting from `settings`,
//! whose code a directory holds from `trust`, a pull request's base
//! through `integrations` (which refuses while GitHub is off), a request
//! counted in `requests`, a note read only while its setting is on, with
//! its ID left out.
//!
//! The diff's folders are the workspace's own (`~/.local/state/switchboard`
//! and `~/.cache/switchboard`), not a `plugins/diff` under them, since its
//! files (`diff-story`, `diff-order`, `diff-base`, `diff-story-ahead`)
//! were there before it was a plugin and a story told is not told twice.
//!
//! `switchboard diff` run outside a pane is answered here too: with no
//! server to ask, a server command has no answer.

use std::path::Path;
use std::sync::Arc;

use switchboard_plugin_api::{Action, Answer, Env, Folders, Host, Pr};

use crate::mux::socket;
use crate::notes;
use crate::{paths, settings, util};

/// The workspace, as the diff asks it.
pub struct Core;

/// The host every program of this binary gives the diff.
pub fn core() -> Arc<dyn Host> {
    Arc::new(Core)
}

/// Says the diff's modules, used here directly (`diff-story`, the story
/// told ahead, a headless Claude's environment), who answers them: every
/// program of the binary does as it starts (`cmd::run`).
pub fn install() {
    switchboard_diff::host::install(core());
}

/// What `plugin::headless` changes in a command's environment.
#[derive(Default)]
struct Changes(Env);

impl crate::setup::plugin::Environment for Changes {
    fn set_var(&mut self, key: &str, value: &str) {
        self.0.set.push((key.to_string(), value.to_string()));
    }
    fn remove_var(&mut self, key: &str) {
        self.0.remove.push(key.to_string());
    }
}

/// The tab's plan note as the story's context: `plan`, a path in the notes
/// directory (never one leaving it), with its stable ID left out, read
/// while the diff_story_plan setting is on, since nothing in a note is
/// sent anywhere unless you turn it on.
fn plan_text(plan: &str) -> Option<String> {
    if !notes::serve::in_notes(plan) || !settings::is_on("diff_story_plan") {
        return None;
    }
    let text = std::fs::read_to_string(notes::store::notes_dir().join(plan)).ok()?;
    Some(notes::id::strip(&text))
}

/// The PR the branch in `dir` has, as GitHub has it: `None` with no PR,
/// GitHub off or out of reach.
fn pr_of(dir: &Path) -> Option<Pr> {
    if !settings::is_on("github") {
        return None;
    }
    let dir = dir.to_path_buf();
    util::block_on(async move {
        crate::integrations::github::pr_for_branch(&dir, "M-d diff: PR base")
            .await
            .ok()
            .flatten()
            .map(|pr| Pr {
                base: pr.base_ref_name,
                number: pr.number,
                head: pr.head_ref_name,
            })
    })
}

/// The settings the diff reads as it tells a story, which it asks again
/// each time rather than once as it opens: Tell it again reads the
/// story's shape among them.
const READ_FRESH: [&str; 5] = [
    "diff_story_model",
    "diff_story",
    "diff_order",
    "diff_story_map",
    "diff_story_talk",
];

impl Host for Core {
    fn ask(&self, action: Action) -> Answer {
        match action {
            Action::Setting { key } => {
                // Asked as a story is told: read fresh, so one changed
                // while the diff is open counts.
                if READ_FRESH.contains(&key.as_str()) {
                    settings::reload_settings();
                }
                match settings::cached(&key) {
                    Some(value) => Answer::Value { value },
                    None => Answer::None,
                }
            }
            Action::Folders => Answer::Folders {
                folders: Folders {
                    state: paths::state_dir(),
                    cache: paths::cache_dir(),
                },
            },
            Action::InPane => Answer::Flag {
                on: socket::in_pane(),
            },
            Action::Command { args } => {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                match socket::ask(&args) {
                    Ok((code, out, err)) => Answer::Ran { code, out, err },
                    Err(_) => Answer::None,
                }
            }
            Action::Copy { text } => {
                let _ = crate::platform::copy(&text);
                Answer::Done
            }
            Action::Pr { dir } => match pr_of(&dir) {
                Some(pr) => Answer::Pr { pr },
                None => Answer::None,
            },
            Action::ReviewedPr { dir } => {
                match crate::integrations::github::pr_number_from_dir(&dir) {
                    Some(number) => Answer::Number { number },
                    None => Answer::None,
                }
            }
            Action::Untrusted { dir } => Answer::Flag {
                on: crate::trust::is_untrusted(&dir),
            },
            Action::Request {
                service,
                protocol,
                source,
            } => {
                crate::requests::count(&service, &protocol, &source);
                Answer::Done
            }
            Action::Fetch {
                dir,
                remote,
                source,
            } => {
                crate::worktree::count_fetch(&dir, &remote, &source);
                Answer::Done
            }
            Action::HeadlessClaude => {
                let mut changes = Changes::default();
                crate::setup::plugin::headless(&mut changes);
                Answer::Env { env: changes.0 }
            }
            Action::Plan { plan } => {
                // Read fresh, so a plan turned off since the diff opened
                // is never sent.
                settings::reload_settings();
                // With none named, the note of the tab the diff runs in,
                // asked of the server only while the setting is on.
                let plan = plan.or_else(|| {
                    settings::is_on("diff_story_plan")
                        .then(|| socket::tab_option(notes::serve::PLAN_NOTE))
                        .flatten()
                });
                match plan.as_deref().and_then(plan_text) {
                    Some(text) => Answer::Text { text },
                    None => Answer::None,
                }
            }
            // Whether a button's rounded ends are drawn, for the diff's row
            // of buttons.
            Action::Draws => Answer::Text {
                text: if crate::mux::graphics::apps_round_ends() {
                    "round".into()
                } else {
                    String::new()
                },
            },
            // Kept as the pane's option, which the server shows over the
            // pane (`Server::pointer_at`); only a shape it knows. The pane
            // is named, so a diff whose pane has just gone (it is told to
            // end, and gives the beam back as it does) changes no other's.
            Action::Pointer { shape } => {
                use crate::ui::pointer::{PANE_OPTION, Pointer};
                // A pane's id (`%<n>`), never a tab's.
                let Some(pane) = std::env::var("SWITCHBOARD_PANE")
                    .ok()
                    .filter(|p| p.starts_with('%'))
                else {
                    return Answer::None;
                };
                let set = |rest: &[&str]| {
                    let args = ["set", "-t", &pane]
                        .iter()
                        .chain(rest)
                        .map(|a| a.to_string())
                        .collect();
                    // Never waited on for long: the diff asks this as it
                    // quits too, before it gives the terminal back.
                    socket::ask_quiet(args, Some(pane.clone()), None)
                };
                let asked = match shape.as_deref().map(Pointer::named) {
                    Some(Some(shape)) => set(&[PANE_OPTION, shape.name()]),
                    Some(None) => return Answer::None,
                    None => set(&["-u", PANE_OPTION]),
                };
                match asked {
                    Some((0, ..)) => Answer::Done,
                    _ => Answer::None,
                }
            }
            // The tab the diff's pane is in marked as needing you, which the
            // server does only while that tab isn't in front
            // (`Server::cmd_ready`).
            Action::Ready { what } => {
                let Some(pane) = std::env::var("SWITCHBOARD_PANE")
                    .ok()
                    .filter(|p| p.starts_with('%'))
                else {
                    return Answer::None;
                };
                let args = vec!["ready".to_string(), what];
                match socket::ask_quiet(args, Some(pane), None) {
                    Some((0, ..)) => Answer::Done,
                    _ => Answer::None,
                }
            }
            // What Notes alone asks is answered where it is loaded
            // (src/notes/host.rs), which hands the rest on to here.
            Action::Settings { .. }
            | Action::Paste
            | Action::PasteImage { .. }
            | Action::Preview { .. }
            | Action::LinkWords { .. }
            | Action::OpenLink { .. }
            | Action::Picture { .. }
            | Action::PictureGone { .. }
            | Action::ItemClaim { .. }
            | Action::Queued
            | Action::Item { .. } => Answer::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use switchboard_plugin_api::{Manifest, Scope};

    #[test]
    fn the_diff_is_loaded_at_this_build_s_protocol_and_its_settings_are_the_schema_s() {
        let manifest: Manifest = crate::diff::plugin().manifest();
        assert_eq!(manifest.check(), Ok(()));
        assert_eq!(manifest.name, "diff");
        // Every setting it says it reads is listed, with that default, name
        // and description, so Settings' rows and the diff's own settings
        // view, alone, say the same; and every setting of the Diff category
        // is one it says. The one that is the diff alone's is in no list of
        // the workspace's.
        let schema = settings::schema();
        for declared in &manifest.settings {
            let listed = schema.iter().find(|o| o.key == declared.key);
            if declared.scope == Scope::Alone {
                assert!(listed.is_none(), "{} is the diff alone's", declared.key);
                assert!(!declared.label.is_empty() && !declared.description.is_empty());
                continue;
            }
            let listed = listed.unwrap_or_else(|| panic!("{} isn't listed", declared.key));
            assert_eq!(listed.default, declared.default, "{}", declared.key);
            assert_eq!(listed.label, declared.label, "{}", declared.key);
            assert_eq!(listed.description, declared.description, "{}", declared.key);
        }
        for option in schema.iter().filter(|o| o.category == "Diff") {
            assert!(
                manifest.default_of(&option.key).is_some(),
                "{} isn't in the diff's manifest",
                option.key
            );
        }
    }

    #[test]
    fn the_workspace_answers_what_the_diff_asks() {
        let host = Core;
        assert_eq!(
            host.setting("diff_story"),
            Some(serde_json::Value::Bool(true))
        );
        assert_eq!(host.setting("no such setting"), None);
        let folders = host.folders().unwrap();
        assert_eq!(folders.state, paths::state_dir());
        assert_eq!(folders.cache, paths::cache_dir());
        // Whose code a directory holds is `trust`'s to say.
        let home = paths::home();
        assert_eq!(host.untrusted(&home), crate::trust::is_untrusted(&home));
        assert!(host.untrusted(&paths::cache_dir().join("pr-review/acme-web/12")));
        // A worktree opened to review a PR carries its number.
        assert_eq!(
            host.reviewed_pr(Path::new("/code/web/pr-12345")),
            Some(12345)
        );
        assert_eq!(host.reviewed_pr(&home), None);
    }

    #[test]
    fn a_plan_note_is_read_only_inside_the_notes_and_with_its_setting_on() {
        // The setting is off by default, so nothing is read.
        assert_eq!(plan_text("Plans/Plan - x.md"), None);
        assert_eq!(plan_text(""), None);
        assert_eq!(
            plan_text("/etc/hosts"),
            None,
            "never a file outside the notes"
        );
        assert_eq!(plan_text("../secret.md"), None);
        assert_eq!(Core.plan(Some("/etc/hosts")), "");
    }
}
