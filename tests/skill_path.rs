//! How Switchboard's skills defer to a repository's own skill of the same
//! name, ported from test/skills.test.js: every skill's opening step,
//! hand-offs between skills, and `switchboard skill-path`, which both use.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{BIN, TempDir};

fn skills_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/skills")
}

/// Every skill's name and SKILL.md text.
fn skills() -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = fs::read_dir(skills_dir())
        .unwrap()
        .flatten()
        .filter_map(|e| {
            let text = fs::read_to_string(e.path().join("SKILL.md")).ok()?;
            Some((e.file_name().to_string_lossy().into_owned(), text))
        })
        .collect();
    all.sort();
    all
}

/// The text after a skill's frontmatter and title, or `None` when it doesn't
/// start with them.
fn after_title(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    let rest = rest[end + 5..].strip_prefix("\n# ")?;
    let title_end = rest.find('\n')?;
    rest[title_end + 1..].strip_prefix('\n')
}

#[test]
fn every_skill_opens_by_running_skill_path_with_its_own_name() {
    let all = skills();
    assert!(!all.is_empty());
    for (name, text) in all {
        let opening = format!(
            "This is Switchboard's own copy, for repositories that have none. Before anything else, run `switchboard skill-path {name}`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here."
        );
        let rest = after_title(&text)
            .unwrap_or_else(|| panic!("{name} starts with its frontmatter and title"));
        assert!(
            rest.starts_with(&format!("{opening}\n")),
            "{name}'s first paragraph is the opening step"
        );
    }
}

#[test]
fn no_skill_names_another_skill_s_skill_md_by_a_fixed_path() {
    // A hand-off that needs Switchboard's own text rather than the
    // repository's copy reads `switchboard skill-path --own`.
    let fixed = regex::Regex::new(r"[^\s`(]*skills/[a-z-]+/SKILL\.md").unwrap();
    for (name, text) in skills() {
        let found: Vec<&str> = fixed.find_iter(&text).map(|m| m.as_str()).collect();
        assert!(
            found.is_empty(),
            "{name} reads other skills through skill-path: {found:?}"
        );
    }
}

#[test]
fn skill_path_finds_the_worktree_s_copy_then_the_main_clone_s_then_the_personal_one() {
    let tmp = TempDir::new("skill-path");
    let home = tmp.path();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .env("HOME", home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@e")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@e")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    let run = |dir: &Path| -> Output {
        Command::new(BIN)
            .args(["skill-path", "demo"])
            .arg(dir)
            .env("HOME", home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap()
    };
    let out = |dir: &Path| String::from_utf8_lossy(&run(dir).stdout).trim().to_string();
    let skill = |root: &Path| {
        let file = root.join(".claude/skills/demo/SKILL.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "# demo\n").unwrap();
        file.to_string_lossy().into_owned()
    };
    let repo = home.join("repo");
    let worktree = home.join("wt");
    let outside = home.join("outside");
    fs::create_dir(&outside).unwrap();
    git(&["init", "-q", repo.to_str().unwrap()]);
    git(&[
        "-C",
        repo.to_str().unwrap(),
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "start",
    ]);
    git(&[
        "-C",
        repo.to_str().unwrap(),
        "worktree",
        "add",
        "-q",
        "-b",
        "feature",
        worktree.to_str().unwrap(),
    ]);

    let none = run(&worktree);
    assert_eq!(none.status.code(), Some(1), "none anywhere");
    assert!(none.stdout.is_empty());

    let personal = skill(home);
    assert_eq!(
        out(&worktree),
        personal,
        "the personal one when the repository has none"
    );
    assert_eq!(
        out(&outside),
        personal,
        "the personal one outside a repository"
    );

    let main = skill(&repo);
    assert_eq!(
        out(&worktree),
        main,
        "the main clone's when the worktree has none"
    );
    assert_eq!(
        out(&repo.join(".claude")),
        main,
        "from inside the main clone"
    );

    let own = skill(&worktree);
    assert_eq!(out(&worktree), own, "the worktree's own first");

    let usage = Command::new(BIN)
        .args(["skill-path", "-x"])
        .output()
        .unwrap();
    assert_eq!(usage.status.code(), Some(2));
}

#[test]
fn every_switchboard_command_a_skill_names_is_one_the_binary_has_and_none_names_tmux() {
    let known = |name: &str| {
        switchboard::cmd::COMMANDS.iter().any(|c| c.name == name)
            || switchboard::mux::commands::NAMES.contains(&name)
    };
    let named =
        regex::Regex::new(r"`switchboard ([a-z][a-z-]*)|^ *switchboard ([a-z][a-z-]*)").unwrap();
    let part =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/switchboard.md"))
            .unwrap();
    let mut texts = skills();
    texts.push(("switchboard.md".into(), part));
    for (name, text) in texts {
        for line in text.lines() {
            for found in named.captures_iter(line) {
                let command = found.get(1).or_else(|| found.get(2)).unwrap().as_str();
                assert!(known(command), "{name} names switchboard {command}");
            }
        }
        assert!(!text.contains("tmux "), "{name} still names tmux");
        assert!(!text.contains("TMUX"), "{name} still reads TMUX");
    }
}

#[test]
fn switchboard_s_own_copy_is_printed_whatever_is_installed() {
    let tmp = TempDir::new("sb-skill-own");
    let run = |name: &str| {
        Command::new(BIN)
            .args(["skill-path", "--own", name])
            .env("HOME", tmp.path())
            .output()
            .unwrap()
    };
    let out = run("ci-loop");
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("When queue-and-clean runs this"), "{text}");
    assert_eq!(run("no-such-skill").status.code(), Some(1));
}
