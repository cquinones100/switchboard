//! This repository's own guard hooks (`.claude/hooks/enforce-push-review.sh`
//! and `enforce-git-hooks-run.sh`), which Claude Code runs before every Bash
//! command here: each is given the JSON Claude Code sends and exits 2 to
//! refuse. They read every line of a command, and refuse when jq is missing
//! or the JSON can't be read, rather than let a command through unread.

use crate::common::TempDir;

use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn hook(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".claude/hooks")
        .join(name)
}

/// The hook's exit code for `input`, with `path` as its PATH.
fn run(name: &str, input: &str, path: &str) -> Option<i32> {
    let mut child = Command::new("/bin/bash")
        .arg(hook(name))
        .env("PATH", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait().unwrap().code()
}

fn command(cmd: &str) -> String {
    serde_json::json!({ "tool_input": { "command": cmd } }).to_string()
}

fn path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

const PUSH: &str = "enforce-push-review.sh";
const COMMIT: &str = "enforce-git-hooks-run.sh";

#[test]
fn the_push_guard_reads_every_line() {
    let refused = |cmd: &str| run(PUSH, &command(cmd), &path()) == Some(2);
    assert!(refused("git push --no-verify origin main"));
    assert!(
        refused("cd .\ngit push --no-verify origin main"),
        "a second line"
    );
    assert!(
        refused("git push \\\n  --no-verify origin main"),
        "a continued line"
    );
    assert!(refused("git -c core.hooksPath=/dev/null push"));
    assert!(refused(
        "printf '%s %s %s\\n' a b c > \"$(git rev-parse --git-path review-loop-clean)\""
    ));
    assert!(refused("cp x .git/review-loop-streak"));
    assert!(!refused("git push origin HEAD:main"));
    assert!(!refused(
        "git commit -m 'Write the review-loop-clean record'"
    ));
    assert!(!refused("switchboard codex-review --recorded"));
}

#[test]
fn the_commit_guard_reads_every_line_and_every_way_to_set_hooks_path() {
    let refused = |cmd: &str| run(COMMIT, &command(cmd), &path()) == Some(2);
    assert!(refused("git commit -n -m x"));
    assert!(
        refused("cd .\ngit commit --no-verify -m x"),
        "a second line"
    );
    assert!(
        refused("git commit \\\n  --no-verify -m x"),
        "a continued line"
    );
    assert!(refused("git -c core.hooksPath=/dev/null commit -m x"));
    assert!(refused(
        "git --config-env=core.hooksPath=NOWHERE commit -m x"
    ));
    assert!(refused(
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.hooksPath GIT_CONFIG_VALUE_0=/dev/null git commit -m x"
    ));
    assert!(refused("git config core.hooksPath /dev/null"));
    assert!(!refused("git config --get core.hooksPath"));
    assert!(!refused("git commit -m 'Mention core.hooksPath and -n'"));
}

#[test]
fn both_refuse_without_jq_or_with_unreadable_json() {
    // A PATH with what the hooks need but jq.
    let tmp = TempDir::new("guard-no-jq");
    for tool in ["cat", "sed", "grep", "tr"] {
        let found = ["/bin", "/usr/bin"]
            .iter()
            .map(|dir| Path::new(dir).join(tool))
            .find(|p| p.exists())
            .unwrap();
        symlink(found, tmp.path().join(tool)).unwrap();
    }
    let no_jq = tmp.path().to_string_lossy().into_owned();
    for name in [PUSH, COMMIT] {
        assert_eq!(
            run(name, &command("ls"), &no_jq),
            Some(2),
            "{name} without jq"
        );
        assert_eq!(
            run(name, "not json", &path()),
            Some(2),
            "{name} with bad JSON"
        );
        assert_eq!(
            run(name, &command("ls"), &path()),
            Some(0),
            "{name} lets ls by"
        );
    }
}
