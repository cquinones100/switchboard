//! How Switchboard's skills defer to a repository's own skill of the same
//! name, ported from test/skills.test.js: every skill's opening step,
//! hand-offs between skills, and `switchboard skill-path`, which both use;
//! and the repository's own project skills (`.claude/skills/`), which
//! never ship.

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
fn skill_path_finds_the_repository_s_copy_then_switchboard_s_personal_one_then_the_plugin_s() {
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

    // Switchboard's plugin's, when there is no other.
    let file = home.join(".local/share/switchboard/plugin/skills/demo/SKILL.md");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "# demo\n").unwrap();
    let plugin = file.to_string_lossy().into_owned();
    assert_eq!(
        out(&worktree),
        plugin,
        "the plugin's when nothing else has it"
    );
    assert_eq!(out(&outside), plugin, "outside a repository too");

    // A skill of yours that only shares the name: Switchboard's own is
    // still the plugin's, so /switchboard:demo and a hand-off to it follow
    // Switchboard's text, not yours.
    let personal = skill(home);
    assert_eq!(out(&worktree), plugin, "not a skill of yours by the name");
    assert_eq!(out(&outside), plugin, "nor outside a repository");

    // Switchboard's, written there the old way.
    let marker = home.join(".claude/skills/demo/.switchboard-skill");
    fs::write(&marker, "SKILL.md\n").unwrap();
    assert_eq!(
        out(&worktree),
        personal,
        "the personal one when it is Switchboard's and the repository has none"
    );
    assert_eq!(
        out(&outside),
        personal,
        "the personal one outside a repository"
    );

    // A copy of Switchboard's made yours, as the guide says to change one:
    // its list deleted, its opening still deferring by skill-path.
    fs::remove_file(&marker).unwrap();
    fs::write(
        &personal,
        "# demo\n\nThis is Switchboard's own copy, for repositories that have none. Before anything else, run `switchboard skill-path demo`: if it prints a path other than this file's, read that file and follow it instead.\n\nMy way.\n",
    )
    .unwrap();
    assert_eq!(
        out(&worktree),
        personal,
        "a copy of Switchboard's made yours"
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

    // A worktree of someone else's PR, as wtpr --untrusted marks it: its own
    // copy is the PR author's, so the main clone's is followed.
    git(&[
        "-C",
        repo.to_str().unwrap(),
        "config",
        "extensions.worktreeConfig",
        "true",
    ]);
    git(&[
        "-C",
        worktree.to_str().unwrap(),
        "config",
        "--worktree",
        "switchboard.untrusted",
        "true",
    ]);
    let passed = run(&worktree);
    assert_eq!(
        String::from_utf8_lossy(&passed.stdout).trim(),
        main,
        "an untrusted worktree's own copy passed over"
    );
    assert!(String::from_utf8_lossy(&passed.stderr).contains("holds an untrusted PR"));
    assert_eq!(out(&repo), main, "the main clone is still its own");

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
    texts.extend(project_skills());
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

/// Switchboard's project skills, in `.claude/skills/`, by name with their
/// SKILL.md: for working on Switchboard itself, so they never ship
/// (tests/setup.rs checks `switchboard setup` leaves them out too).
fn project_skills() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(".claude/skills");
    let mut all: Vec<(String, String)> = fs::read_dir(dir)
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

#[test]
fn every_project_skill_is_named_after_its_folder_has_a_description_and_never_ships() {
    let all = project_skills();
    assert!(all.iter().any(|(name, _)| name == "review-pr"));
    let shipped = switchboard::setup::skills::shipped();
    let named = regex::Regex::new(r"`((?:src|assets|tests|docs|\.claude)/[^`\s]+)`").unwrap();
    for (name, text) in all {
        let frontmatter = text
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .map(|(front, _)| front)
            .unwrap_or_else(|| panic!("{name} starts with its frontmatter"));
        assert!(
            frontmatter.lines().any(|l| l == format!("name: {name}")),
            "{name}'s name is its folder's"
        );
        assert!(
            frontmatter.lines().any(|l| l
                .strip_prefix("description: ")
                .is_some_and(|d| !d.trim().is_empty())),
            "{name} has a description"
        );
        assert!(
            !shipped.contains_key(name.as_str()),
            "{name} isn't one of the skills the binary carries"
        );
        // What it names of the repository is there, so a move in the code
        // that leaves it behind fails here.
        for path in named.captures_iter(&text).map(|c| c[1].to_string()) {
            assert!(
                Path::new(env!("CARGO_MANIFEST_DIR")).join(&path).exists(),
                "{name} names {path}, which isn't there"
            );
        }
    }
}

#[test]
fn plan_item_asks_where_the_item_goes_the_top_in_order_of_difficulty_or_the_bottom() {
    let all = skills();
    let (_, text) = all.iter().find(|(name, _)| name == "plan-item").unwrap();
    assert!(
        text.lines().any(|l| l == "disable-model-invocation: true"),
        "it reads your note, so only you start it"
    );
    for option in [
        "**Top of the list**",
        "**In order of difficulty**",
        "**Bottom of the list**",
    ] {
        assert!(text.contains(option), "{option}");
    }
    // The shape autopilot reads: an open choice is a "Decision
    // needed" heading in the plan and "(needs a choice" on the item.
    for shape in [
        "## Decision needed",
        "(needs a choice: ",
        "Plan: [[Plan - <name>]]",
    ] {
        assert!(text.contains(shape), "{shape}");
    }
}

// test/skills.test.js's "a tab of autopilot lands without asking
// only once leak-check finds nothing, and its review loop fixes mediums
// without asking".
#[test]
fn a_tab_of_the_work_a_note_mode_lands_without_asking_only_once_leak_check_finds_nothing() {
    let all = skills();
    let text = |name: &str| {
        all.iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.clone())
            .unwrap()
    };
    let queue = text("queue-and-clean");
    let step = |start: &str| {
        queue
            .lines()
            .find(|l| l.starts_with(start))
            .unwrap_or_else(|| panic!("no {start}"))
            .to_string()
    };
    let check = "switchboard show @autopilot_land";
    let step7 = step("7. **Ask before pushing**");
    let leak = step7
        .find("switchboard leak-check <base>..HEAD")
        .expect("step 7 runs leak-check");
    let reads = step7.find(check).expect("step 7 reads the tab's switch");
    assert!(leak < reads, "leak-check runs first");
    assert!(
        regex::Regex::new(r"`leak-check` exited 0, the landing ran both the tests and the review loop .*two clean rounds of this very change .*take the answer as \*\*Push to <name>\*\* without asking")
            .unwrap()
            .is_match(&step7),
        "{step7}"
    );
    assert!(
        queue.contains("<lock> autolanded <short hash>"),
        "it says so after landing"
    );
    let step11 = step("11. **Push**");
    let again = step11
        .find("read `@autopilot_land` again")
        .expect("it reads the switch again");
    let push = step11.find("git push origin HEAD:<name>").unwrap();
    assert!(again < push, "just before the push");
    let review = text("review-loop");
    assert!(
        review.contains(check),
        "the review loop reads the same switch"
    );
    assert!(review.contains("fix a `medium` one you agree with as a `high` one is"));
}

#[test]
fn review_loop_reviews_someone_elses_pr_from_outside_its_worktree_and_only_reports() {
    let all = skills();
    let (_, text) = all.iter().find(|(name, _)| name == "review-loop").unwrap();
    let section = text
        .split("## Someone else's PR, from outside its worktree\n")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the section");
    for said in [
        "`switchboard codex-review -C <worktree> --pr <number> --base <base> --notes <notes-file>`",
        "`--text --no-textconv --no-ext-diff`",
        "`switchboard try --sealed`",
        "Nothing is fixed, committed, pushed, or edited on GitHub",
        "Write no `review-loop-clean` record.",
        "**For the author**",
    ] {
        assert!(section.contains(said), "{said}");
    }
    // The flags it names are codex-review's own.
    let usage = Command::new(BIN)
        .args(["codex-review", "--help"])
        .output()
        .unwrap();
    let usage = String::from_utf8_lossy(&usage.stderr);
    for flag in [
        "-C <dir>",
        "--pr <number|url>",
        "--base <ref>",
        "--notes <file>",
    ] {
        assert!(usage.contains(flag), "codex-review takes {flag}: {usage}");
    }
}

#[test]
fn review_pr_runs_none_of_the_prs_code_outside_the_sealed_try() {
    let all = project_skills();
    let (_, text) = all.iter().find(|(name, _)| name == "review-pr").unwrap();
    assert!(
        text.lines().any(|l| l == "disable-model-invocation: true"),
        "it checks out someone else's code, so only you start it"
    );
    for said in [
        "`switchboard worktree-from-pr --untrusted <pr>`",
        "`switchboard pr-risk <worktree> <base>`",
        "`--text --no-textconv --no-ext-diff`",
        "`switchboard skill-path review-loop`",
        "`switchboard try --sealed <worktree> --run cargo test`",
        "on Linux `switchboard try --sealed` refuses",
        "never `cd` a shell into it",
    ] {
        assert!(text.contains(said), "{said}");
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
