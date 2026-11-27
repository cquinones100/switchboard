//! M-p's "Try this tab's branch": `switchboard try` for the tab's worktree
//! in a new tab in the background, its build shown there, then the try's
//! workspace attached in that tab, so trying a branch never means leaving
//! the workspace for a terminal of its own. The tab runs `try --print` and
//! attaches with the environment it prints, as a terminal of your own would
//! (`eval "$(switchboard try --print)"`, then `switchboard`), since plain
//! `try` refuses to attach inside a pane. `try` itself keeps refusing
//! someone else's code, and a worktree `wtpr --untrusted` made is refused
//! here first, by its path alone, since it runs only sealed (src/trust.rs).
//! New with the Rust port; no tmux-version file.

use std::path::{Path, PathBuf};

use crate::mux::PaneId;
use crate::mux::server::{ClientId, Opened, Server};

/// The command the try's tab runs for worktree `wt`, with `bin` the
/// workspace's own binary: the build's output and any refusal stay on the
/// tab's screen until Enter, and once built the try's own binary attaches.
pub fn command(bin: &Path, wt: &Path) -> Vec<String> {
    let script = concat!(
        "env=$(\"$1\" try --print \"$2\") || ",
        "{ printf '\\nPress Enter to close this tab.'; read -r _; exit 1; }; ",
        "eval \"$env\"; exec \"$HOME/.local/bin/switchboard\"",
    );
    vec![
        "sh".into(),
        "-c".into(),
        script.into(),
        "sh".into(),
        bin.to_string_lossy().into_owned(),
        wt.to_string_lossy().into_owned(),
    ]
}

/// The top of the checkout `dir` is in, when it is a Switchboard clone or
/// one of its worktrees: what `try` builds. Files alone, no git process.
pub fn checkout_of(dir: &Path) -> Option<PathBuf> {
    let top = dir.ancestors().find(|d| d.join(".git").exists())?;
    crate::dev::is_switchboard_clone(top).then(|| top.to_path_buf())
}

/// `label` with the `#<PR> ` it may lead with left out, since the try's tab,
/// in the same worktree, gets its own.
fn without_pr(label: &str) -> &str {
    label
        .strip_prefix('#')
        .and_then(|rest| rest.split_once(' '))
        .filter(|(number, _)| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
        .map_or(label, |(_, rest)| rest)
}

impl Server {
    /// The palette's "Try this tab's branch", pressed in `pane`.
    pub(crate) fn try_branch(&mut self, client: ClientId, pane: Option<PaneId>) {
        let dir = pane.and_then(|p| self.pane_dir(p));
        let Some(wt) = dir.as_deref().and_then(checkout_of) else {
            self.say(
                Some(client),
                "This tab has no Switchboard worktree of its own, so there's nothing to try",
            );
            return;
        };
        if crate::trust::in_review_dir(&wt) {
            self.say(
                Some(client),
                "This tab's worktree holds someone else's PR, which runs only in switchboard try --sealed",
            );
            return;
        }
        let bin = match crate::util::exe() {
            Ok(bin) => bin,
            Err(e) => {
                self.say(
                    Some(client),
                    &format!("Couldn't find Switchboard's binary: {e}"),
                );
                return;
            }
        };
        let name = pane
            .and_then(|p| self.panes.get(&p).map(|slot| slot.tab))
            .map(|tab| self.tab_label_without_pr_title(tab))
            .unwrap_or_default();
        let Some(tab) = self.tab_at_end(client, &wt, command(&bin, &wt), Opened::Background) else {
            return;
        };
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.opts
                .insert("@tab_name".into(), format!("Try: {}", without_pr(&name)));
        }
        self.dirty_tab(tab);
        self.name_tab(tab);
        self.say(
            Some(client),
            &format!(
                "Trying {} in a new tab: it attaches once it's built",
                crate::paths::tilde(&wt)
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tabs_name_leaves_out_its_pr() {
        assert_eq!(without_pr("#82 Try it"), "Try it");
        assert_eq!(without_pr("#fix the build"), "#fix the build");
        assert_eq!(without_pr("Notes"), "Notes");
    }

    #[test]
    fn its_command_prints_the_trys_environment_then_attaches() {
        let words = command(Path::new("/bin/switchboard"), Path::new("/code/wt"));
        assert_eq!(&words[..2], ["sh", "-c"]);
        assert!(words[2].contains("\"$1\" try --print \"$2\""));
        assert!(words[2].ends_with("exec \"$HOME/.local/bin/switchboard\""));
        assert_eq!(&words[3..], ["sh", "/bin/switchboard", "/code/wt"]);
    }
}
