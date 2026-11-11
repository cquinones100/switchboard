//! `claude-here`, ported from test/claude-here.test.js: the sessions found
//! for a directory, laid out in a temporary projects directory as Claude
//! Code's `~/.claude/projects` is, so nothing reads your own; what a
//! transcript's end says; how long ago reads; and `--print` run as the
//! program, in a temporary HOME.

use crate::common;

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use common::{BIN, TempDir};
use serde_json::json;
use switchboard::cmd::claude_here::{
    encode_prefix, find_sessions, format_relative_time, local_date, read_session_tail,
};

static NEXT: AtomicU32 = AtomicU32::new(1);

/// A session id shaped as Claude Code's, different every time.
fn uuid() -> String {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{:08x}-0000-4000-8000-{:012x}", std::process::id(), n)
}

fn ms(iso: &str) -> i64 {
    iso.parse::<jiff::Timestamp>().unwrap().as_millisecond()
}

/// Lays out a session that ran in `cwd` on `day`, having started in
/// `started_in`, opened again after it was last used (as a restore does,
/// which says nothing of when it was used), each older than the last.
struct Projects<'a> {
    dir: &'a Path,
    opened: i64,
}

impl Projects<'_> {
    fn session(&mut self, cwd: &str, day: &str, started_in: Option<&str>) -> String {
        let started_in = started_in.unwrap_or(cwd);
        let id = uuid();
        let dir = self.dir.join(encode_prefix(started_in));
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(cwd).unwrap();
        fs::create_dir_all(started_in).unwrap();
        let file = dir.join(format!("{id}.jsonl"));
        let lines = [
            json!({ "type": "user", "cwd": started_in, "sessionId": id, "timestamp": "2025-12-01T00:00:00Z" }),
            json!({ "type": "user", "cwd": cwd, "sessionId": id, "timestamp": format!("{day}T00:00:00Z") }),
            json!({ "type": "cost-state" }),
        ];
        fs::write(&file, lines.map(|l| l.to_string()).join("\n")).unwrap();
        self.opened -= 60_000;
        let at = UNIX_EPOCH + Duration::from_millis(self.opened as u64);
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(at)
            .unwrap();
        id
    }
}

fn ids(found: &[switchboard::cmd::claude_here::Session]) -> Vec<String> {
    found.iter().map(|s| s.id.clone()).collect()
}

#[test]
fn finds_the_target_s_sessions_last_active_first_with_each_one_s_own_directory() {
    let tmp = TempDir::new("claude-here");
    let projects_dir = tmp.path().join("projects");
    let work = tmp.path().join("work").to_string_lossy().into_owned();
    let repo = format!("{work}/repo");
    let mut p = Projects {
        dir: &projects_dir,
        opened: ms("2026-12-31T00:00:00Z"),
    };
    let oldest = p.session(&repo, "2026-01-01", None);
    let nested = p.session(&format!("{repo}/packages/app"), "2026-03-01", None);
    let middle = p.session(&repo, "2026-02-01", None);
    // Started in a folder of the repository, then moved up to its top:
    // resumed where it started.
    let moved_up = p.session(&repo, "2026-02-20", Some(&format!("{repo}/packages/web")));
    // Started in the repository under its old name, and in a folder of it
    // since, so resumed from the repository's.
    let renamed = p.session(&format!("{repo}/lib"), "2026-02-15", Some(&repo));
    fs::write(
        projects_dir
            .join(encode_prefix(&repo))
            .join(format!("{renamed}.jsonl")),
        [
            json!({ "type": "user", "cwd": format!("{work}.repo"), "timestamp": "2025-12-01T00:00:00Z" }),
            json!({ "type": "user", "cwd": format!("{repo}/lib"), "timestamp": "2026-02-15T00:00:00Z" }),
        ]
        .map(|l| l.to_string())
        .join("\n"),
    )
    .unwrap();
    p.session(&format!("{work}/repo-other"), "2026-04-01", None);
    let worktree = p.session(
        &format!("{repo}/.claude/worktrees/feature-x"),
        "2026-05-01",
        None,
    );
    // Started in the repository, then moved into one of its worktrees.
    p.session(
        &format!("{repo}/.claude/worktrees/feature-y"),
        "2026-06-01",
        Some(&repo),
    );
    let repo_project = projects_dir.join(encode_prefix(&repo));
    fs::write(
        repo_project.join("agent-a1b2c3.jsonl"),
        format!("{}\n", json!({ "cwd": repo })),
    )
    .unwrap();
    // A transcript that can't be read: a link to nothing, by a session's name.
    std::os::unix::fs::symlink(
        Path::new(&work).join("gone.jsonl"),
        repo_project.join(format!("{}.jsonl", uuid())),
    )
    .unwrap();

    let found = find_sessions(&repo, &projects_dir);
    assert_eq!(
        ids(&found),
        vec![
            nested.clone(),
            moved_up.clone(),
            renamed.clone(),
            middle.clone(),
            oldest.clone()
        ],
        "a sibling under the same lossy name, a nested worktree, one that moved into one, a subagent's transcript and one that can't be read left out"
    );
    assert_eq!(
        found[0].cwd,
        format!("{repo}/packages/app"),
        "each resumed where it ran"
    );
    assert_eq!(
        found[1].cwd,
        format!("{repo}/packages/web"),
        "one that moved up resumed where it started"
    );
    assert_eq!(
        found[2].cwd, repo,
        "one whose starting folder was renamed resumed where its transcript is"
    );
    assert_eq!(found[0].last_active_ms, ms("2026-03-01T00:00:00Z"));
    assert_eq!(
        ids(&find_sessions(
            &format!("{repo}/.claude/worktrees/feature-x"),
            &projects_dir
        )),
        vec![worktree],
        "the worktree's own, asked from there"
    );
    assert!(find_sessions(&format!("{work}/none"), &projects_dir).is_empty());
    fs::remove_dir_all(format!("{repo}/packages/app")).unwrap();
    assert_eq!(
        ids(&find_sessions(&repo, &projects_dir)),
        vec![moved_up, renamed, middle, oldest],
        "one whose folder is gone isn't offered"
    );
    assert!(find_sessions("/work/repo", &projects_dir.join("missing")).is_empty());
}

#[test]
fn reads_a_session_s_last_directory_activity_and_prompt_skipping_what_claude_code_adds() {
    let tmp = TempDir::new("claude-here-tail");
    let file = tmp.path().join("s.jsonl");
    let lines = [
        json!({ "type": "permission-mode", "permissionMode": "auto" }).to_string(),
        "not json at all".to_string(),
        json!({ "type": "user", "cwd": "/work/repo", "timestamp": "2026-06-01T10:00:00Z", "message": { "content": "first prompt" } }).to_string(),
        json!({ "type": "user", "cwd": "/work/repo/lib", "timestamp": "2026-06-01T11:00:00Z", "message": { "content": [{ "type": "text", "text": "follow-up\n  \u{1b}[2J\u{1b}[Hquestion\u{7}" }] } }).to_string(),
        json!({ "type": "user", "isMeta": true, "message": { "content": "meta caveat" } }).to_string(),
        json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "x", "content": "output" }] } }).to_string(),
        json!({ "type": "user", "message": { "content": "<command-name>/clear</command-name>" } }).to_string(),
        json!({ "type": "cost-state" }).to_string(),
    ];
    fs::write(&file, lines.join("\n")).unwrap();
    let tail = read_session_tail(&file);
    assert_eq!(tail.cwd.as_deref(), Some("/work/repo/lib"));
    assert_eq!(tail.last_active_ms, Some(ms("2026-06-01T11:00:00Z")));
    assert_eq!(tail.last_message.as_deref(), Some("follow-up question"));

    // Far enough back to need more than the first piece read from the end.
    let pad = json!({ "type": "cost-state", "pad": "x".repeat(300 * 1024) });
    fs::write(&file, format!("{}\n{pad}", lines.join("\n"))).unwrap();
    assert_eq!(
        read_session_tail(&file).last_message.as_deref(),
        Some("follow-up question"),
        "found past the first piece read"
    );

    fs::write(&file, format!("{}\n", lines[0])).unwrap();
    assert_eq!(read_session_tail(&file), Default::default());
    assert_eq!(
        read_session_tail(&tmp.path().join("missing.jsonl")),
        Default::default()
    );
}

#[test]
fn says_how_long_ago_a_session_was_active() {
    let now = ms("2026-06-12T12:00:00Z");
    let minute = 60_000;
    let said: Vec<String> = [0.5f64, 5.0, 180.0, 1800.0, 4.0 * 1440.0]
        .iter()
        .map(|m| format_relative_time(now - (m * minute as f64) as i64, now))
        .collect();
    assert_eq!(
        said,
        ["just now", "5m ago", "3h ago", "yesterday", "4d ago"]
    );
    let month = now - 30 * 1440 * minute;
    assert_eq!(format_relative_time(month, now), local_date(month));
}

#[test]
fn print_lists_the_sessions_of_the_repository_it_runs_in() {
    let tmp = TempDir::new("claude-here-print");
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap()
                .status
                .success()
        );
    };
    git(&["init", "-q"]);
    let repo_s = repo.to_string_lossy().into_owned();
    let mut p = Projects {
        dir: &home.join(".claude/projects"),
        opened: ms("2026-12-31T00:00:00Z"),
    };
    let id = p.session(&repo_s, "2026-03-01", None);
    let run = |dir: &Path| {
        Command::new(BIN)
            .args(["claude-here", "--print"])
            .current_dir(dir)
            .env("HOME", &home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap()
    };
    let out = run(&repo);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let fields: Vec<&str> = stdout.trim().split('\t').collect();
    assert_eq!(fields[0], id);
    assert_eq!(fields[1], "2026-03-01T00:00:00.000Z");
    assert!(fields[2].ends_with(&format!("{id}.jsonl")));

    let empty = tmp.path().join("empty");
    fs::create_dir_all(&empty).unwrap();
    let none = run(&empty);
    assert_eq!(none.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&none.stderr).contains("No Claude sessions found for"));
}

#[test]
fn a_worktree_of_someone_elses_pr_is_never_resumed_in() {
    let tmp = TempDir::new("here-untrusted");
    let home = tmp.path().join("home");
    // Checked out by wtpr --untrusted, in the review folder, with a session
    // there all the same.
    let review = home.join(".cache/switchboard/pr-review/o-r/12345");
    fs::create_dir_all(&review).unwrap();
    let ok = Command::new("git")
        .args(["init", "-q"])
        .arg(&review)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .unwrap();
    assert!(ok.success());
    let mut p = Projects {
        dir: &home.join(".claude/projects"),
        opened: ms("2026-12-31T00:00:00Z"),
    };
    p.session(&review.to_string_lossy(), "2026-03-01", None);
    let out = Command::new(BIN)
        .args(["claude-here", "--print"])
        .current_dir(&review)
        .env("HOME", &home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("Claude isn't started there"), "{said}");
    assert!(out.stdout.is_empty(), "no session listed");
}
