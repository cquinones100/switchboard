//! `switchboard tickets [<dir>]`: where the repository at `<dir>` (the
//! current directory by default) keeps its work, which `/shape` files new
//! work in, on the first line: `github` (its issues), `linear`, or `notes`
//! (a note's checklist with a plan note each), repos.json's `tickets`
//! (`repos::tickets_at`), `linear` when it isn't set, as `/shape` filed
//! only Linear tickets before it had a key. Then, whatever the first line
//! (`/shape --github` files an issue in any repository), a second line
//! `autopilot-label <label>` when autopilot takes this repository's issues
//! (autopilot.json's `repo` is its main clone, its `sources` take issues
//! and its issues are on, `Mode::issues_on`), naming the label an issue carries for it to be taken
//! (`issueLabel`), so an issue `/shape` files joins autopilot's queue. It
//! reads two files and runs git once, and fails when repos.json can't be
//! read or `<dir>` isn't in a repository. No tmux-version file is behind it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::autopilot::mode::Mode;

/// The label autopilot takes the repository's issues by, when it takes
/// them from the repository whose main clone is `clone`.
pub fn autopilot_label(mode: &Mode, clone: &Path) -> Option<String> {
    let same = |a: &Path, b: &Path| {
        a.canonicalize()
            .ok()
            .is_some_and(|a| Some(a) == b.canonicalize().ok())
    };
    (mode.sources.issues() && !mode.repo.as_os_str().is_empty() && same(&mode.repo, clone))
        .then(|| mode.issue_label.clone())
}

pub fn main(args: Vec<String>) -> ExitCode {
    if let Some(bad) = args.iter().find(|a| a.starts_with('-')) {
        eprintln!("tickets: unknown option {bad}\nusage: switchboard tickets [<dir>]");
        return ExitCode::from(2);
    }
    let dir = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let tickets = match crate::repos::tickets_at(&dir) {
        Ok(t) => t.unwrap_or_else(|| "linear".into()),
        Err(e) => {
            eprintln!("tickets: {e}");
            return ExitCode::from(1);
        }
    };
    println!("{tickets}");
    let mode = Mode::read(&crate::paths::state_dir());
    let label = super::work::main_clone(&dir)
        .filter(|_| mode.issues_on())
        .and_then(|clone| autopilot_label(&mode, &clone));
    if let Some(label) = label {
        println!("autopilot-label {label}");
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autopilot::mode::Sources;

    #[test]
    fn the_label_is_named_only_when_autopilot_takes_issues_from_this_clone() {
        let dir = std::env::temp_dir().join(format!("sb-tickets-{}", std::process::id()));
        let (here, other) = (dir.join("here"), dir.join("other"));
        std::fs::create_dir_all(&here).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let mut mode = Mode {
            repo: here.clone(),
            sources: Sources::Both,
            issue_label: "ready".into(),
            ..Mode::default()
        };
        assert_eq!(autopilot_label(&mode, &here).as_deref(), Some("ready"));
        assert_eq!(autopilot_label(&mode, &other), None, "another repository's");
        mode.sources = Sources::Note;
        assert_eq!(autopilot_label(&mode, &here), None, "the note alone");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
