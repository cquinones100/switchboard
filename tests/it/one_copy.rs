//! The 1.0 refactor's guardrails: one way to do each thing, so a change
//! that adds another copy of a helper the refactor is bringing down to one
//! fails here. Each check has an allowlist of the copies there are today,
//! every entry naming the refactor issue that removes it, and the issue
//! empties its entries as it lands; nothing is added to one.
//!
//! - git's wrapper: `fn git(` only in plugin-api's `sdk` (issue 399).
//! - Claude's transcripts: read only by `agent::transcript` (issue 400).
//! - Mission control: a view acts on an item through its menus, never a
//!   key of its own (issue 396).

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

/// Where git's one wrapper will live (issue 399).
const GIT_HOME: &str = "crates/plugin-api/src/sdk";

/// The files with a `fn git(` of their own today.
const GIT_COPIES: &[(&str, u32)] = &[
    ("crates/diff/src/git.rs", 399),
    ("crates/diff/tests/alone.rs", 399),
    ("src/cmd/clone_build.rs", 399),
    ("src/cmd/codex_review.rs", 399),
    ("src/cmd/item_done.rs", 399),
    ("src/cmd/land_push.rs", 399),
    ("src/cmd/land_squash.rs", 399),
    ("src/cmd/leak_check.rs", 399),
    ("src/cmd/pr_risk.rs", 399),
    ("src/cmd/try_.rs", 399),
    ("src/cmd/worktree_from_note.rs", 399),
    ("src/cmd/wt_jump.rs", 399),
    ("src/cmd/wt_revive.rs", 399),
    ("src/day/standup.rs", 399),
    ("src/demo/mod.rs", 399),
    ("src/files/mod.rs", 399),
    ("src/integrations/github.rs", 399),
    ("src/merge_queue/train.rs", 399),
    ("src/mux/behind.rs", 399),
    ("src/mux/latest.rs", 399),
    ("src/mux/main_clone.rs", 399),
    ("src/relay/places.rs", 399),
    ("src/tabs/git.rs", 399),
    ("src/trust.rs", 399),
    ("src/worktree_sweep/mod.rs", 399),
    ("src/worktree_sweep/reviews.rs", 399),
];

/// Where Claude's transcripts will be read (issue 400).
const TRANSCRIPT_HOME: &str = "src/agent/transcript.rs";

/// The files that read a transcript's entries themselves today.
const TRANSCRIPT_COPIES: &[(&str, u32)] = &[
    ("src/cmd/claude_here.rs", 400),
    ("src/cmd/claude_reply.rs", 400),
    ("src/cmd/wt_revive.rs", 400),
    ("src/history/transcript.rs", 400),
    ("src/phone/turns.rs", 400),
    ("src/relay/files.rs", 400),
    ("src/relay/milestones.rs", 400),
    ("src/relay/summaries.rs", 400),
    ("src/relay/tabs.rs", 400),
    ("src/sidebar/claude.rs", 400),
    ("src/views/conversation.rs", 400),
];

/// Mission control's boxes, which take answers rather than act on a view's
/// item.
const MISSION_BOXES: &[&str] = &[
    "src/mission/answer.rs",
    "src/mission/ask.rs",
    "src/mission/find_key.rs",
    "src/mission/remind_box.rs",
    "src/mission/reply_box.rs",
    "src/mission/views_box.rs",
];

/// The keys every view shares: moving, mission control's own, and its
/// filter's and menu's. `n`, `p`, `c` and `u` are among them only with
/// Ctrl.
const MISSION_KEYS: &str = "hjklHLJKgGwWmqiv/?[] 0123456789";
const MISSION_CTRL_KEYS: &str = "npcu";

/// The functions whose keys act on a view's item today.
const MISSION_ITEM_KEYS: &[(&str, &str, u32)] = &[("src/mission/serve.rs", "routines_key", 396)];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every Rust file of src/ and crates/, by its path from the repository,
/// with its text.
fn sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    rust_files(&root.join("crates"), &mut files);
    files
        .iter()
        .map(|f| {
            let name = f.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            (name, fs::read_to_string(f).unwrap())
        })
        .collect()
}

/// A file's code before its test module.
fn before_tests(text: &str) -> &str {
    let tests = Regex::new(r"(?m)^#\[cfg\(test\)\]\s*\nmod ").unwrap();
    tests.find(text).map_or(text, |m| &text[..m.start()])
}

fn allowed<'a>(list: impl IntoIterator<Item = &'a str>, name: &str) -> bool {
    list.into_iter().any(|a| a == name)
}

/// The files that would break a rule, but the allowlist's, as lines for a
/// failure.
fn new_copies(rule: &str, found: Vec<String>, list: &[(&str, u32)]) -> Vec<String> {
    found
        .into_iter()
        .filter(|name| !allowed(list.iter().map(|(a, _)| *a), name))
        .map(|name| format!("{name}: {rule}"))
        .collect()
}

/// Allowlist entries that no longer match anything, which the issue that
/// removed them should have taken out.
fn stale(found: &[String], list: &[(&str, u32)]) -> Vec<String> {
    list.iter()
        .filter(|(a, _)| !found.iter().any(|f| f == a))
        .map(|(a, issue)| {
            format!("{a}: no longer a copy, so take it out of the allowlist (issue {issue})")
        })
        .collect()
}

fn git_copies(files: &[(String, String)]) -> Vec<String> {
    let def = Regex::new(r"\bfn git\s*[(<]").unwrap();
    files
        .iter()
        .filter(|(name, text)| !name.starts_with(GIT_HOME) && def.is_match(text))
        .map(|(name, _)| name.clone())
        .collect()
}

fn transcript_copies(files: &[(String, String)]) -> Vec<String> {
    let entry_type = Regex::new(
        r#"Some\("(assistant|user)"\)|"(assistant|user)"\s*(\|[^=]*)?=>|"type":"(assistant|user)""#,
    )
    .unwrap();
    let transcript = Regex::new(r"(?i)transcript|jsonl").unwrap();
    files
        .iter()
        .filter(|(name, text)| {
            let code = before_tests(text);
            name != TRANSCRIPT_HOME && entry_type.is_match(code) && transcript.is_match(code)
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// Each `<file> <function>` in mission control whose `KeyCode::Char` is
/// none of the keys every view shares.
fn mission_item_keys(files: &[(String, String)]) -> Vec<String> {
    let char_key = Regex::new(r"KeyCode::Char\(\s*'([^']+)'((?:\s*\|\s*'[^']+')*)").unwrap();
    let quoted = Regex::new(r"'([^']+)'").unwrap();
    let function = Regex::new(r"\bfn (\w+)").unwrap();
    let mut found = Vec::new();
    for (name, text) in files {
        if !name.starts_with("src/mission/") || allowed(MISSION_BOXES.iter().copied(), name) {
            continue;
        }
        let mut current = String::new();
        for line in before_tests(text).lines() {
            if let Some(f) = function.captures(line) {
                current = f[1].to_string();
            }
            let ctrl = line.to_lowercase().contains("ctrl");
            let item_key = char_key.find_iter(line).any(|m| {
                quoted.captures_iter(m.as_str()).any(|c| {
                    let key = &c[1];
                    let shared = key.chars().count() == 1
                        && (MISSION_KEYS.contains(key) || ctrl && MISSION_CTRL_KEYS.contains(key));
                    !shared
                })
            });
            let at = format!("{name} {current}");
            if item_key && !found.contains(&at) {
                found.push(at);
            }
        }
    }
    found
}

#[test]
fn git_has_one_wrapper() {
    let found = git_copies(&sources());
    let mut bad = new_copies(
        &format!("a new `fn git(`; use git's one wrapper in {GIT_HOME}"),
        found.clone(),
        GIT_COPIES,
    );
    bad.extend(stale(&found, GIT_COPIES));
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn transcripts_have_one_reader() {
    let found = transcript_copies(&sources());
    let mut bad = new_copies(
        "a new reader of Claude's transcripts; read them through agent::transcript",
        found.clone(),
        TRANSCRIPT_COPIES,
    );
    bad.extend(stale(&found, TRANSCRIPT_COPIES));
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn mission_views_act_through_menus() {
    let found = mission_item_keys(&sources());
    let list: Vec<String> = MISSION_ITEM_KEYS
        .iter()
        .map(|(f, func, _)| format!("{f} {func}"))
        .collect();
    let mut bad: Vec<String> = found
        .iter()
        .filter(|at| !list.contains(at))
        .map(|at| {
            format!("{at}: a key that acts on a view's item; make it an option of the item's menu")
        })
        .collect();
    bad.extend(
        MISSION_ITEM_KEYS
            .iter()
            .zip(&list)
            .filter(|(_, at)| !found.contains(at))
            .map(|((_, _, issue), at)| {
                format!("{at}: no longer acts on an item, so take it out of the allowlist (issue {issue})")
            }),
    );
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn a_new_copy_fails() {
    let files = vec![
        ("src/new.rs".to_string(), "fn git(args: &[&str]) {}".to_string()),
        (
            "src/agent/new.rs".to_string(),
            r#"fn read(transcript: &Path) { if e.get("type").and_then(Value::as_str) == Some("assistant") {} }"#
                .to_string(),
        ),
        (
            "src/mission/new.rs".to_string(),
            "fn new_key(k: Key) { match k.code { KeyCode::Char('j') => {} KeyCode::Char('d') => {} } }".to_string(),
        ),
        (
            "src/mission/moves.rs".to_string(),
            "fn moves(k: Key) { KeyCode::Char('n') if ctrl => {} }\n#[cfg(test)]\nmod tests { fn t() { KeyCode::Char('d'); } }"
                .to_string(),
        ),
        (format!("{GIT_HOME}/git.rs"), "pub fn git(args: &[&str]) {}".to_string()),
    ];
    assert_eq!(git_copies(&files), ["src/new.rs"]);
    assert_eq!(transcript_copies(&files), ["src/agent/new.rs"]);
    assert_eq!(mission_item_keys(&files), ["src/mission/new.rs new_key"]);
}
