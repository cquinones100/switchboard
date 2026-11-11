//! `switchboard leak-check`: what it finds in a range's added lines and
//! commit messages, run as queue-and-clean runs it, against a throwaway
//! repository and HOME, and which kinds a line holds. Ported from
//! test/leak-check.test.js. It is a file of `tests/`, whose ticket IDs and
//! PR numbers leak-check takes as made up, so it passes leak-check itself.

use crate::common;

use std::process::{Command, Output, Stdio};

use common::{BIN, TempDir};
use switchboard::cmd::leak_check::{Hit, hits, hits_in, is_test, leave_out};

struct World {
    dir: TempDir,
}

impl World {
    fn new() -> World {
        let w = World {
            dir: TempDir::new("leak"),
        };
        std::fs::create_dir_all(w.home().join(".config/switchboard")).unwrap();
        std::fs::create_dir_all(w.repo()).unwrap();
        w.git(&["init", "-q"]);
        w.git(&["symbolic-ref", "HEAD", "refs/heads/main"]);
        std::fs::write(w.repo().join("a.md"), "Ticket FOO-9 and #4567 were here.\n").unwrap();
        w.git(&["add", "."]);
        w.git(&["commit", "-q", "-m", "start"]);
        w
    }

    fn home(&self) -> std::path::PathBuf {
        self.dir.path().join("home")
    }

    fn repo(&self) -> std::path::PathBuf {
        self.dir.path().join("repo")
    }

    fn command(&self, program: &str) -> Command {
        let mut cmd = Command::new(program);
        cmd.current_dir(self.repo())
            .env("HOME", self.home())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@e")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@e")
            .env_remove("SWITCHBOARD")
            .env_remove("SWITCHBOARD_PANE")
            .stdin(Stdio::null());
        cmd
    }

    fn git(&self, args: &[&str]) -> String {
        let out = self.command("git").args(args).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn base(&self) -> String {
        self.git(&["rev-list", "--max-parents=0", "HEAD"])
    }

    fn leak_check(&self, args: &[&str]) -> Output {
        self.command(BIN)
            .arg("leak-check")
            .args(args)
            .output()
            .unwrap()
    }

    /// `files` committed over the first commit with `message`, and
    /// leak-check's run on what that adds: its exit code and output.
    fn check(&self, files: &[(&str, Option<&str>)], message: &str) -> (i32, String) {
        let base = self.base();
        self.git(&["reset", "-q", "--hard", &base]);
        for (file, text) in files {
            let path = self.repo().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            match text {
                Some(text) => std::fs::write(&path, text).unwrap(),
                None => std::fs::remove_file(&path).unwrap(),
            }
        }
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
        let run = self.leak_check(&[&format!("{base}..HEAD")]);
        (
            run.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&run.stdout).trim().to_string(),
        )
    }
}

/// A token's shape, put together here so this file holds none whole, and
/// passes leak-check itself.
fn token() -> String {
    format!("ghp_{}", "a1B2".repeat(8))
}

#[test]
fn a_hit_names_its_file_and_line_and_a_message_its_commit() {
    let w = World::new();
    let (code, out) = w.check(
        &[("lib/x.js", Some("one\n// Fixed in ENG-123 today\n"))],
        "Add a thing",
    );
    assert_eq!(code, 1);
    assert_eq!(out, "lib/x.js:2: ticket ID: ENG-123");
    let (code, out) = w.check(&[], "Fix ENG-77\n\nAs #9876 asked");
    assert_eq!(code, 1);
    assert!(
        regex::Regex::new(
            r"(?m)^commit [0-9a-f]+: ticket ID: ENG-77$\n^commit [0-9a-f]+: PR number: #9876$"
        )
        .unwrap()
        .is_match(&out),
        "{out}"
    );
    assert_eq!(
        w.check(
            &[],
            "Add a thing\n\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
        ),
        (0, String::new()),
        "Claude's trailer"
    );
}

#[test]
fn a_word_from_leak_check_txt_is_caught_as_a_whole_word_in_any_case() {
    let w = World::new();
    std::fs::write(
        w.home().join(".config/switchboard/leak-check.txt"),
        "# my words\n\nAcmeCorp\nintranet.acme.test\n",
    )
    .unwrap();
    let (code, out) = w.check(
        &[(
            "notes.md",
            Some("Built for acmecorp at intranet.acme.test\nNot acmecorporation\n"),
        )],
        "Add a thing",
    );
    assert_eq!(code, 1);
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            "notes.md:1: listed word: acmecorp",
            "notes.md:1: listed word: intranet.acme.test"
        ]
    );
}

#[test]
fn a_line_one_commit_adds_and_a_later_removes_counts_as_does_a_merged_side_branchs() {
    let w = World::new();
    let base = w.base();
    std::fs::write(w.repo().join("b.md"), format!("key {}\n", token())).unwrap();
    w.git(&["add", "-A"]);
    w.git(&["commit", "-q", "-m", "Add b"]);
    std::fs::remove_file(w.repo().join("b.md")).unwrap();
    w.git(&["add", "-A"]);
    w.git(&["commit", "-q", "-m", "Remove b"]);
    let run = w.leak_check(&[&format!("{base}..HEAD")]);
    assert_eq!(run.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&run.stdout).starts_with("b.md:1: token: ghp_a1B2"));

    // A side branch merged in, its added line removed before the merge.
    w.git(&["reset", "-q", "--hard", &base]);
    w.git(&["checkout", "-q", "-b", "side"]);
    std::fs::write(w.repo().join("c.md"), format!("key {}\n", token())).unwrap();
    w.git(&["add", "-A"]);
    w.git(&["commit", "-q", "-m", "Add c"]);
    std::fs::remove_file(w.repo().join("c.md")).unwrap();
    w.git(&["add", "-A"]);
    w.git(&["commit", "-q", "-m", "Remove c"]);
    w.git(&["checkout", "-q", "main"]);
    w.git(&["reset", "-q", "--hard", &base]);
    w.git(&["commit", "-q", "--allow-empty", "-m", "Main moves"]);
    w.git(&["merge", "-q", "--no-ff", "-m", "Merge side", "side"]);
    w.git(&["branch", "-q", "-D", "side"]);
    let run = w.leak_check(&[&format!("{base}..HEAD")]);
    assert_eq!(run.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&run.stdout)
            .lines()
            .any(|l| l.starts_with("c.md:1: token: ghp_a1B2"))
    );
}

#[test]
fn a_file_marked_binary_is_still_read_and_removed_lines_are_left_out() {
    let w = World::new();
    let line = format!("TOKEN={}\n", token());
    let (code, out) = w.check(
        &[
            (".gitattributes", Some(".env -diff\n")),
            (".env", Some(&line)),
        ],
        "Add a thing",
    );
    assert_eq!(code, 1);
    assert!(
        out.lines()
            .any(|l| l.starts_with(".env:1: token: ghp_a1B2")),
        "{out}"
    );
    assert_eq!(
        w.check(&[("a.md", None)], "Add a thing"),
        (0, String::new()),
        "what was there before the range, removed, is left out"
    );
}

#[test]
fn it_exits_2_when_it_cant_look() {
    let w = World::new();
    assert_eq!(w.leak_check(&[]).status.code(), Some(2), "no range");
    let list = w.home().join(".config/switchboard/leak-check.txt");
    std::fs::create_dir_all(&list).unwrap();
    assert_eq!(
        w.leak_check(&["HEAD..HEAD"]).status.code(),
        Some(2),
        "a word list that can't be read stops it"
    );
    std::fs::remove_dir(&list).unwrap();
    let bad = w.leak_check(&["nope..HEAD"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stderr).starts_with("leak-check: "));
}

// A home path and an address, put together here so this file holds none
// whole, and passes leak-check itself.
fn home_path() -> String {
    ["", "Users", "somebody"].join("/")
}

fn email(user: &str) -> String {
    [user, "company.io"].join("@")
}

fn kinds(line: &str, test: bool) -> Vec<String> {
    hits_in(line, test, &[])
        .into_iter()
        .map(|(k, t)| format!("{k}: {t}"))
        .collect()
}

// test/leak-check.test.js's "each kind is caught in an added line".
#[test]
fn each_kind_is_caught_in_a_line() {
    let token_of = |prefix: &str| format!("{prefix}{}", "a1B2".repeat(8));
    let cases: Vec<(String, String)> = vec![
        ("Fixed in ENG-123 today".into(), "ticket ID: ENG-123".into()),
        ("See #4321 for why".into(), "PR number: #4321".into()),
        ("Related PR: #4321".into(), "PR number: #4321".into()),
        (
            "Follow up PLATFORM-42".into(),
            "ticket ID: PLATFORM-42".into(),
        ),
        ("Follow up A-42".into(), "ticket ID: A-42".into()),
        ("Follow up B2B-42".into(), "ticket ID: B2B-42".into()),
        (
            "git checkout ENG-456-fix-login".into(),
            "ticket ID: ENG-456".into(),
        ),
        (
            format!("Contact {}", email("me")),
            format!("email address: {}", email("me")),
        ),
        ("{ \"pr\": \"#456\" }".into(), "PR number: #456".into()),
        (
            "Fix background polling from #456".into(),
            "PR number: #456".into(),
        ),
        (
            format!("cd {}/code", home_path()),
            format!("home path: {}", home_path()),
        ),
        (
            format!("Mail {}", email("jane.doe")),
            format!("email address: {}", email("jane.doe")),
        ),
        (
            format!("key {}", token_of("ghp_")),
            "token: ghp_a1B2…".into(),
        ),
        (
            format!("key {}", token_of("sk-")),
            "token: sk-a1B2a…".into(),
        ),
        (
            format!("key {}", token_of("xoxb-")),
            "token: xoxb-a1B…".into(),
        ),
        (
            format!("key {}", token_of("xapp-")),
            "token: xapp-a1B…".into(),
        ),
    ];
    for (line, expected) in cases {
        assert_eq!(kinds(&line, false), [expected], "{line}");
    }
}

// test/leak-check.test.js's "made-up IDs are allowed in tests, and the docs'
// placeholders anywhere".
#[test]
fn made_up_ids_pass_in_tests_and_the_docs_placeholders_anywhere() {
    assert!(kinds("assert(ticket === 'ENG-123' && pr === '#4321')", true).is_empty());
    assert!(is_test("tests/x.rs") && is_test("lib/x.test.js") && !is_test("src/x.rs"));
    assert!(
        kinds(
            "Like ABC-123, #123 or #12345, in UTF-8 with SHA-256, MPL-2.0, M-1 and C-2",
            false
        )
        .is_empty(),
        "placeholders and standards"
    );
    assert!(
        kinds(
            ".a { color: #202124; } .b { background: \"#000000\" }",
            false
        )
        .is_empty()
    );
    assert!(kinds("local bg = \"#000000\"", false).is_empty(), "colors");
    assert!(
        kinds("git@github.com:me/x and noreply@anthropic.com", false).is_empty(),
        "SSH and noreply addresses"
    );
    assert!(kinds("issue#456, a&#456; and x/#456 aren't PRs", false).is_empty());
    assert!(
        kinds("xENG-123 and ENG-123x", false).is_empty(),
        "within words"
    );
    assert_eq!(
        kinds(&format!("cd {}", home_path()), true),
        [format!("home path: {}", home_path())],
        "a home path counts in a test too"
    );
}

#[test]
fn a_listed_word_is_found_whole_in_any_case() {
    let words = vec!["AcmeCorp".to_string(), "intranet.acme.test".to_string()];
    let found: Vec<String> = hits_in(
        "Built for acmecorp at intranet.acme.test, not acmecorporation",
        false,
        &words,
    )
    .into_iter()
    .map(|(k, t)| format!("{k}: {t}"))
    .collect();
    assert_eq!(
        found,
        ["listed word: acmecorp", "listed word: intranet.acme.test"]
    );
    assert!(hits_in("acmecorporation", false, &words).is_empty());
}

#[test]
fn added_lines_are_read_with_their_file_and_line_and_messages_without_claudes_trailer() {
    let diff = "diff --git a/lib/x.js b/lib/x.js\n--- a/lib/x.js\n+++ b/lib/x.js\n@@ -1,0 +1,2 @@\n+one\n+// Fixed in ENG-123\n-// removed ENG-9\ndiff --git a/gone.md b/gone.md\n--- a/gone.md\n+++ /dev/null\n@@ -1 +0,0 @@\n-ENG-8\n";
    let log = "abc1234\0Fix ENG-77\n\nCo-Authored-By: Claude Opus <noreply@anthropic.com>\n\0\ndef5678\0Plain\n\0";
    let shown: Vec<String> = hits(diff, log, &[]).into_iter().map(|h| h.shown).collect();
    assert_eq!(
        shown,
        [
            "lib/x.js:2: ticket ID: ENG-123",
            "commit abc1234: ticket ID: ENG-77"
        ]
    );
}

fn hit(shown: &str, file: Option<&str>, whole: &str) -> Hit {
    Hit {
        shown: shown.to_string(),
        file: file.map(str::to_string),
        kind: "ticket ID",
        whole: whole.to_string(),
    }
}

#[test]
fn a_hit_the_bases_copy_of_its_file_has_is_left_out_and_any_other_kept() {
    let found = vec![
        hit("src/a.rs:9: ticket ID: AB-1", Some("src/a.rs"), "AB-1"),
        hit("src/a.rs:10: ticket ID: AB-7", Some("src/a.rs"), "AB-7"),
        hit("src/new.rs:1: ticket ID: AB-1", Some("src/new.rs"), "AB-1"),
        hit("src/b.rs:1: ticket ID: AB-2", Some("src/b.rs"), "AB-2"),
        hit("src/a.rs:11: ticket ID: AB-12", Some("src/a.rs"), "AB-12"),
        hit("commit abc1234: ticket ID: AB-1", None, "AB-1"),
    ];
    let mut asked = Vec::new();
    let (kept, left) = leave_out(found, &[], |file| {
        asked.push(file.to_string());
        match file {
            "src/a.rs" => Some("let id = \"AB-1\"; // AB-123\n".to_string()),
            "src/b.rs" => Some("AB-1 lives in a.rs\n".to_string()),
            _ => None,
        }
    });
    let shown = |hits: Vec<Hit>| hits.into_iter().map(|h| h.shown).collect::<Vec<_>>();
    assert_eq!(
        shown(kept),
        [
            "src/a.rs:10: ticket ID: AB-7",
            "src/new.rs:1: ticket ID: AB-1",
            "src/b.rs:1: ticket ID: AB-2",
            "src/a.rs:11: ticket ID: AB-12",
            "commit abc1234: ticket ID: AB-1",
        ],
        "one new to the file, a new file's, one the base has in another file, one the base has only within a longer ID, a message's"
    );
    assert_eq!(shown(left), ["src/a.rs:9: ticket ID: AB-1"]);
    assert_eq!(
        asked,
        ["src/a.rs", "src/new.rs", "src/b.rs"],
        "each file read once"
    );
}

#[test]
fn a_rewritten_line_whose_id_the_base_has_passes_and_a_new_id_does_not() {
    let w = World::new();
    std::fs::create_dir_all(w.repo().join("src")).unwrap();
    std::fs::write(w.repo().join("src/a.rs"), "one\nlet id = \"AB-1\";\ntwo\n").unwrap();
    w.git(&["add", "-A"]);
    w.git(&["commit", "-q", "-m", "Add a"]);
    let base = w.git(&["rev-parse", "HEAD"]);

    std::fs::write(
        w.repo().join("src/a.rs"),
        "one\nlet ids = [\"AB-1\"];\ntwo\n",
    )
    .unwrap();
    w.git(&["commit", "-q", "-am", "Rewrite it"]);
    let run = w.leak_check(&[&format!("{base}..HEAD")]);
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "");
    assert_eq!(
        String::from_utf8_lossy(&run.stderr).trim(),
        format!("leak-check: 1 already on {} left out", &base[..8])
    );
    let all = w.leak_check(&["--all", &format!("{base}..HEAD")]);
    assert_eq!(all.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&all.stdout).trim(),
        format!("src/a.rs:2: ticket ID: AB-1 (on {})", &base[..8])
    );

    std::fs::write(w.repo().join("src/a.rs"), "let id = \"AB-7\";\none\ntwo\n").unwrap();
    w.git(&["commit", "-q", "-am", "Change it"]);
    let run = w.leak_check(&[&format!("{base}..HEAD")]);
    assert_eq!(run.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "src/a.rs:1: ticket ID: AB-7"
    );
}

#[test]
fn a_new_token_beside_one_the_base_has_is_kept() {
    let old = format!("ghp_{}", "a1B2".repeat(8));
    let new = format!("ghp_{}", "a1B2".repeat(7) + "zzzz");
    let diff = format!(
        "diff --git a/k.md b/k.md\n--- a/k.md\n+++ b/k.md\n@@ -1 +1 @@\n+keys {old} {new}\n"
    );
    let found = hits(&diff, "", &[]);
    assert_eq!(found.len(), 2, "two tokens shown alike are both kept");
    let (kept, _) = leave_out(found, &[], |_| Some(format!("key {old}\n")));
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].whole, new);
}

#[test]
fn a_listed_word_the_base_has_in_another_case_is_left_out_but_not_one_inside_a_longer_word() {
    let words = vec!["AcmeCorp".to_string()];
    let diff = "diff --git a/n.md b/n.md\n--- a/n.md\n+++ b/n.md\n@@ -1 +1 @@\n+for acmecorp\n";
    let (kept, left) = leave_out(hits(diff, "", &words), &words, |_| {
        Some("Built for AcmeCorp\n".to_string())
    });
    assert!(kept.is_empty() && left.len() == 1);
    let (kept, _) = leave_out(hits(diff, "", &words), &words, |_| {
        Some("not acmecorporation\n".to_string())
    });
    assert_eq!(kept.len(), 1);
}

#[test]
fn a_file_whose_name_git_ends_with_a_tab_or_quotes_is_named_as_it_is() {
    let diff = "diff --git a/release notes.md b/release notes.md\n--- a/release notes.md\t\n+++ b/release notes.md\t\n@@ -1 +1 @@\n+AB-1\ndiff --git \"a/x\\\"y.md\" \"b/x\\\"y.md\"\n+++ \"b/x\\\"y.md\"\n@@ -1 +1 @@\n+AB-2\n";
    let files: Vec<Option<String>> = hits(diff, "", &[]).into_iter().map(|h| h.file).collect();
    assert_eq!(
        files,
        [
            Some("release notes.md".to_string()),
            Some("x\"y.md".to_string())
        ]
    );
}

#[test]
fn an_added_line_that_looks_like_a_file_header_is_read_as_a_line() {
    let diff = "diff --git a/new.md b/new.md\n--- /dev/null\n+++ b/new.md\n@@ -0,0 +1,2 @@\n+++ b/src/a.rs\n+AB-1\n";
    let found = hits(diff, "", &[]);
    let shown: Vec<&str> = found.iter().map(|h| h.shown.as_str()).collect();
    assert_eq!(shown, ["new.md:2: ticket ID: AB-1"]);
    let (kept, _) = leave_out(found, &[], |file| {
        (file == "src/a.rs").then(|| "AB-1\n".to_string())
    });
    assert_eq!(
        kept.len(),
        1,
        "src/a.rs's base copy leaves out nothing in new.md"
    );
}

#[test]
fn a_file_is_named_from_the_diffs_own_prefixes_whatever_git_is_set_to() {
    let w = World::new();
    w.git(&["config", "diff.noprefix", "true"]);
    std::fs::write(w.repo().join("example.md"), "AB-7\n").unwrap();
    w.git(&["add", "-A"]);
    w.git(&["commit", "-q", "-m", "Add example"]);
    let base = w.git(&["rev-parse", "HEAD"]);
    std::fs::create_dir_all(w.repo().join("b")).unwrap();
    std::fs::write(w.repo().join("b/example.md"), "AB-7\n").unwrap();
    w.git(&["add", "-A"]);
    w.git(&["commit", "-q", "-m", "Add b/example"]);
    let run = w.leak_check(&[&format!("{base}..HEAD")]);
    assert_eq!(run.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "b/example.md:1: ticket ID: AB-7"
    );
}

#[test]
fn a_file_is_named_from_the_repositorys_top_whatever_git_is_set_to() {
    let w = World::new();
    w.git(&["config", "diff.relative", "true"]);
    std::fs::write(w.repo().join("example.md"), "AB-7\n").unwrap();
    w.git(&["add", "-A"]);
    w.git(&["commit", "-q", "-m", "Add example"]);
    let base = w.git(&["rev-parse", "HEAD"]);
    std::fs::create_dir_all(w.repo().join("src")).unwrap();
    std::fs::write(w.repo().join("src/example.md"), "AB-7\n").unwrap();
    w.git(&["add", "-A"]);
    w.git(&["commit", "-q", "-m", "Add src/example"]);
    let run = w
        .command(BIN)
        .current_dir(w.repo().join("src"))
        .args(["leak-check", &format!("{base}..HEAD")])
        .output()
        .unwrap();
    assert_eq!(run.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "src/example.md:1: ticket ID: AB-7"
    );
}

#[test]
fn a_single_revision_leaves_nothing_out() {
    let w = World::new();
    std::fs::write(w.repo().join("k.md"), "AB-7\n").unwrap();
    w.git(&["add", "-A"]);
    w.git(&["commit", "-q", "-m", "Add k"]);
    std::fs::write(w.repo().join("k.md"), "AB-7 again\n").unwrap();
    w.git(&["commit", "-q", "-am", "Change k"]);
    let run = w.leak_check(&["HEAD"]);
    assert_eq!(run.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&run.stderr).is_empty());
}
