//! The worktree commands, each run as `switchboard <name>` against a
//! throwaway "remote" (a bare repository) and a clone of it in a temporary
//! directory, with a temporary HOME, so nothing reaches your own
//! repositories; GitHub and Linear are a small HTTP server of the test's
//! own (their addresses moved there by `SWITCHBOARD_GITHUB_GRAPHQL` and
//! `SWITCHBOARD_LINEAR_API`). Ported from test/worktree-from-branch.test.js,
//! worktree-from-pr.test.js and the worktree-from-note tests.

use crate::common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use common::http::{self, Reply};
use common::{BIN, TempDir};
use serde_json::{Value, json};

/// A temporary home with a repository world in it.
struct World {
    dir: TempDir,
    env: Vec<(String, String)>,
}

impl World {
    fn new(prefix: &str) -> World {
        let dir = TempDir::new(prefix);
        let home = dir.path().to_path_buf();
        let env = vec![
            ("HOME", home.display().to_string()),
            (
                "GIT_CONFIG_GLOBAL",
                home.join(".gitconfig").display().to_string(),
            ),
            ("GIT_AUTHOR_NAME", "t".into()),
            ("GIT_AUTHOR_EMAIL", "t@e".into()),
            ("GIT_COMMITTER_NAME", "t".into()),
            ("GIT_COMMITTER_EMAIL", "t@e".into()),
            (
                "SWITCHBOARD_STATE_DIR",
                home.join("state").display().to_string(),
            ),
            ("NOTES_DIR", home.join("Notes").display().to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        std::fs::write(home.join(".gitconfig"), "").unwrap();
        World { dir, env }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn set(&mut self, key: &str, value: &str) {
        self.env.retain(|(k, _)| k != key);
        self.env.push((key.into(), value.into()));
    }

    fn command(&self, program: &str) -> Command {
        let mut cmd = Command::new(program);
        for var in [
            "SWITCHBOARD",
            "SWITCHBOARD_PANE",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "LINEAR_API_KEY",
            "WT_PROMPT",
            "WTN_NOTE",
        ] {
            cmd.env_remove(var);
        }
        cmd.envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null());
        cmd
    }

    /// git in a directory, its output trimmed; panics when it fails.
    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = self
            .command("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn git_fails(&self, dir: &Path, args: &[&str]) -> bool {
        !self
            .command("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    }

    /// `switchboard <args>` in a directory.
    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(BIN)
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap()
    }

    /// As `run`, failing the test unless it succeeds.
    fn ok(&self, cwd: &Path, args: &[&str]) {
        let out = self.run(cwd, args);
        assert!(
            out.status.success(),
            "switchboard {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A new repository on `main`, bare or not, as `git init -b main` makes
    /// one on a git newer than some machines have.
    fn init(&self, dir: &Path, bare: bool) {
        let mut args = vec!["init", "-q"];
        if bare {
            args.push("--bare");
        }
        args.push(dir.to_str().unwrap());
        self.git(self.dir.path(), &args);
        self.git(dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    }

    /// A clone of `from`, on main even while `from` is empty, as a git that
    /// defaults to master would leave it.
    fn clone_into(&self, from: &str, dir: &Path) {
        self.git(
            self.dir.path(),
            &["clone", "-q", from, dir.to_str().unwrap()],
        );
        if self.git_fails(dir, &["rev-parse", "--verify", "-q", "HEAD"]) {
            self.git(dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        }
    }

    /// A bare origin with a first commit on main, and a clone of it.
    fn origin_and_clone(&self) -> (PathBuf, PathBuf) {
        let origin = self.path("origin.git");
        let clone = self.path("clone");
        self.init(&origin, true);
        self.clone_into(origin.to_str().unwrap(), &clone);
        self.git(&clone, &["commit", "-q", "--allow-empty", "-m", "start"]);
        self.git(&clone, &["push", "-q", "origin", "main"]);
        (origin, clone)
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// GitHub's or Linear's GraphQL endpoint, of the test's own and as strict as
/// the real one about how it is asked (tests/common/http.rs: GitHub's token
/// `test-token`, Linear's key `lin_test`), answering every query with what
/// `answer` makes of its body, and keeping what it was sent.
struct Fake {
    http: http::Fake,
}

impl Fake {
    fn github(answer: fn(&Value) -> Value) -> Fake {
        Fake {
            http: http::Fake::github("test-token", move |request| {
                (request.path() == "/graphql").then(|| Reply::json(200, answer(&request.json())))
            }),
        }
    }

    fn linear(answer: fn(&Value) -> Value) -> Fake {
        Fake {
            http: http::Fake::linear("lin_test", move |request| {
                Some(Reply::json(200, answer(&request.json())))
            }),
        }
    }

    /// The GraphQL endpoint.
    fn graphql(&self) -> String {
        self.http.graphql_url()
    }

    /// The queries it answered.
    fn queries(&self) -> Vec<String> {
        self.http
            .seen()
            .iter()
            .filter(|r| r.method == "POST" && r.path() == "/graphql")
            .map(|r| r.query())
            .collect()
    }
}

// ── worktree-from-branch ────────────────────────────────────────────────────

#[test]
fn a_branch_that_exists_nowhere_is_made_off_the_remotes_main_as_it_is_now_with_no_upstream() {
    let w = World::new("wtb");
    let (origin, clone) = w.origin_and_clone();
    // main moves on the remote after this clone last fetched, and the clone
    // sits on a branch of its own.
    let other = w.path("other");
    w.clone_into(origin.to_str().unwrap(), &other);
    w.git(&other, &["commit", "-q", "--allow-empty", "-m", "newer"]);
    w.git(&other, &["push", "-q", "origin", "main"]);
    let newest = w.git(&other, &["rev-parse", "HEAD"]);
    w.git(&clone, &["checkout", "-q", "-b", "elsewhere"]);
    w.git(
        &clone,
        &["commit", "-q", "--allow-empty", "-m", "unrelated"],
    );

    let cd = w.path("cd");
    w.ok(
        &clone,
        &[
            "worktree-from-branch",
            "--no-claude",
            "--cd-file",
            cd.to_str().unwrap(),
            "feature/login",
        ],
    );
    let worktree = PathBuf::from(read(&cd).trim());
    assert_eq!(
        worktree,
        clone.join(".claude/worktrees/feature-login"),
        "flat, the slash made a dash"
    );
    assert_eq!(
        w.git(&worktree, &["branch", "--show-current"]),
        "feature/login"
    );
    assert_eq!(
        w.git(&worktree, &["rev-parse", "HEAD"]),
        newest,
        "off the remote's main, fetched first"
    );
    assert!(
        w.git_fails(&worktree, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
        "with no upstream until its first push"
    );
}

#[test]
fn a_branch_only_the_remote_has_is_checked_out_tracking_it_and_a_rerun_reuses_the_worktree() {
    let w = World::new("wtb");
    let (_, clone) = w.origin_and_clone();
    w.git(&clone, &["checkout", "-q", "-b", "feat"]);
    w.git(&clone, &["commit", "-q", "--allow-empty", "-m", "feature"]);
    let last = w.git(&clone, &["rev-parse", "HEAD"]);
    w.git(&clone, &["push", "-q", "origin", "feat"]);
    w.git(&clone, &["checkout", "-q", "main"]);
    w.git(&clone, &["branch", "-q", "-D", "feat"]);

    let cd = w.path("cd");
    w.ok(
        &clone,
        &[
            "worktree-from-branch",
            "--no-claude",
            "--cd-file",
            cd.to_str().unwrap(),
            "feat",
        ],
    );
    let worktree = PathBuf::from(read(&cd).trim());
    assert_eq!(
        w.git(&worktree, &["rev-parse", "HEAD"]),
        last,
        "at the remote's branch"
    );
    assert_eq!(
        w.git(&worktree, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
        "origin/feat",
        "tracking it"
    );

    w.ok(
        &worktree,
        &[
            "worktree-from-branch",
            "--no-claude",
            "--cd-file",
            cd.to_str().unwrap(),
            "feat",
        ],
    );
    assert_eq!(
        read(&cd).trim(),
        worktree.to_str().unwrap(),
        "the same worktree, from inside it"
    );
}

#[test]
fn a_branch_checked_out_in_the_main_clone_is_refused_and_a_name_git_rejects_never_reaches_git_worktree()
 {
    let w = World::new("wtb");
    let clone = w.path("clone");
    w.init(&clone, false);
    w.git(&clone, &["commit", "-q", "--allow-empty", "-m", "start"]);
    let main = w.run(&clone, &["worktree-from-branch", "--no-claude", "main"]);
    assert_eq!(main.status.code(), Some(1));
    assert!(stderr(&main).contains("already checked out in your main clone"));
    let bad = w.run(&clone, &["worktree-from-branch", "--no-claude", "-bad"]);
    assert!(
        stderr(&bad).contains("not a valid branch name"),
        "{}",
        stderr(&bad)
    );
    assert!(!clone.join(".claude").exists(), "no worktree made");
}

#[test]
fn every_worktree_command_prints_its_shell_function() {
    let w = World::new("wt-init");
    for (command, function) in [
        ("worktree-from-ticket", "wt()"),
        ("worktree-from-pr", "wtpr()"),
        ("worktree-from-branch", "wtb()"),
        ("worktree-from-note", "wtn()"),
        ("worktree-resume", "wtr()"),
    ] {
        for shell in ["zsh", "bash"] {
            let out = w.run(w.dir.path(), &[command, "init", shell]);
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success(), "{command} init {shell}");
            assert!(text.starts_with(function), "{command}: {text}");
            assert!(
                text.contains(&format!("{command} --no-claude --cd-file")),
                "{text}"
            );
        }
        let out = w.run(w.dir.path(), &[command, "init", "fish"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(stderr(&out).contains("init requires a shell"));
    }
    // The function is good shell: bash defines it, and runs it as far as
    // the command, which says what went wrong outside a repository.
    let script = format!(
        "eval \"$('{BIN}' worktree-from-branch init bash)\"; cd /; wtb some-branch; echo rc=$?"
    );
    let out = w.command("bash").args(["-c", &script]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("rc=1"), "{text} {}", stderr(&out));
    assert!(
        stderr(&out).contains("not inside a git repository"),
        "{}",
        stderr(&out)
    );
}

// ── worktree-from-pr ────────────────────────────────────────────────────────

fn pr_answer(body: &Value) -> Value {
    let query = body["query"].as_str().unwrap_or("");
    if query.contains("viewer") {
        return json!({ "data": { "viewer": { "login": "t" } } });
    }
    let owner = body["variables"]["owner"].as_str().unwrap_or("");
    let number = body["variables"]["number"].as_u64().unwrap_or(0);
    let pr = match (owner, number) {
        ("o", 7) => {
            json!({ "number": 7, "title": "Feature", "author": { "login": "t" }, "headRefName": "feat", "headRepository": { "name": "r" }, "headRepositoryOwner": { "login": "o" }, "isCrossRepository": false, "state": "MERGED", "url": "https://github.com/o/r/pull/7", "baseRefName": "main", "body": "" })
        }
        ("other", 5) => {
            json!({ "number": 5, "title": "Fix", "author": { "login": "t" }, "headRefName": "fix", "headRepository": { "name": "thing" }, "headRepositoryOwner": { "login": "other" }, "isCrossRepository": false, "state": "OPEN", "url": "https://github.com/other/thing/pull/5", "baseRefName": "main", "body": "" })
        }
        ("acme", 9) => {
            json!({ "number": 9, "title": "Fix", "author": { "login": "alice", "createdAt": "2015-01-01T00:00:00Z" }, "headRefName": "main", "headRepository": { "name": "widgets" }, "headRepositoryOwner": { "login": "alice" }, "isCrossRepository": true, "maintainerCanModify": true, "state": "OPEN", "url": "https://github.com/acme/widgets/pull/9", "baseRefName": "main", "body": "", "changedFiles": 4, "additions": 4, "deletions": 4 })
        }
        _ => Value::Null,
    };
    json!({ "data": { "repository": { "pullRequest": pr } } })
}

fn github_world(prefix: &str) -> (World, Fake) {
    let mut w = World::new(prefix);
    let fake = Fake::github(pr_answer);
    w.set("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql());
    w.set("SWITCHBOARD_GITHUB_API", &fake.http.url);
    w.set("GH_TOKEN", "test-token");
    (w, fake)
}

#[test]
fn a_merged_pr_whose_branch_the_remote_deleted_opens_from_the_commits_github_keeps_for_it() {
    let (w, fake) = github_world("wtpr");
    let origin = w.path("origin.git");
    // The clone's origin reads as GitHub's o/r, reached through insteadOf.
    std::fs::write(
        w.path(".gitconfig"),
        format!(
            "[url \"{}\"]\n\tinsteadOf = git@github.com:o/r.git\n",
            origin.display()
        ),
    )
    .unwrap();
    let clone = w.path("clone");
    w.init(&origin, true);
    w.clone_into("git@github.com:o/r.git", &clone);
    w.git(&clone, &["commit", "-q", "--allow-empty", "-m", "start"]);
    w.git(&clone, &["push", "-q", "origin", "main"]);
    w.git(&clone, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(clone.join("f.txt"), "feature\n").unwrap();
    w.git(&clone, &["add", "f.txt"]);
    w.git(&clone, &["commit", "-q", "-m", "feature"]);
    let last = w.git(&clone, &["rev-parse", "HEAD"]);
    w.git(&clone, &["push", "-q", "origin", "feat"]);
    // Merged on GitHub: its commits kept under refs/pull/7/head, its branch
    // deleted, and no trace of it left in this clone.
    w.git(&origin, &["update-ref", "refs/pull/7/head", &last]);
    w.git(&clone, &["push", "-q", "origin", "--delete", "feat"]);
    w.git(&clone, &["checkout", "-q", "main"]);
    w.git(&clone, &["branch", "-q", "-D", "feat"]);
    w.git(&clone, &["fetch", "-q", "--prune"]);

    let cd = w.path("cd");
    w.ok(
        &clone,
        &[
            "worktree-from-pr",
            "--no-claude",
            "--cd-file",
            cd.to_str().unwrap(),
            "7",
        ],
    );
    let text = read(&cd);
    let mut lines = text.lines();
    let worktree = PathBuf::from(lines.next().unwrap());
    let prompt = lines.next().unwrap();
    assert_eq!(worktree.file_name().unwrap(), "pr-7-feat");
    assert!(
        prompt.starts_with("Pick up PR #7 by t. It is merged, so say that first. "),
        "Claude starts by saying where the PR stands: {prompt}"
    );
    assert!(
        prompt.ends_with("Don't change anything until I say."),
        "and waits, on a PR of your own nothing about pushing: {prompt}"
    );
    assert_eq!(
        w.git(&worktree, &["rev-parse", "HEAD"]),
        last,
        "at the PR's last commit"
    );
    assert_eq!(w.git(&worktree, &["branch", "--show-current"]), "feat");
    assert!(
        w.git_fails(&worktree, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
        "with no upstream"
    );
    assert!(
        fake.queries()
            .iter()
            .any(|q| q.contains("pullRequest(number"))
    );
}

#[test]
fn a_pr_url_from_another_repository_opens_in_a_clone_of_that_one_found_among_yours_or_is_refused() {
    let (w, _fake) = github_world("wtpr");
    // Where you are: a clone of acme/widgets, away from your home's top level.
    let here = w.path("work/widgets");
    std::fs::create_dir_all(&here).unwrap();
    w.git(&here, &["init", "-q"]);
    w.git(
        &here,
        &["remote", "add", "origin", "git@github.com:acme/widgets.git"],
    );
    // Right under your home: a clone of other/thing, whose GitHub is a bare
    // repository holding the PR's branch, reached through insteadOf.
    let origin = w.path("thing-origin.git");
    let thing = w.path("thing");
    w.init(&origin, true);
    std::fs::write(
        w.path(".gitconfig"),
        format!(
            "[url \"{}\"]\n\tinsteadOf = git@github.com:other/thing.git\n",
            origin.display()
        ),
    )
    .unwrap();
    w.clone_into(origin.to_str().unwrap(), &thing);
    w.git(&thing, &["commit", "-q", "--allow-empty", "-m", "start"]);
    w.git(&thing, &["push", "-q", "origin", "main", "main:fix"]);
    w.git(
        &thing,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:other/thing.git",
        ],
    );

    let cd = w.path("cd");
    w.ok(
        &here,
        &[
            "worktree-from-pr",
            "--no-claude",
            "--cd-file",
            cd.to_str().unwrap(),
            "https://github.com/other/thing/pull/5",
        ],
    );
    assert_eq!(
        read(&cd).lines().next().unwrap(),
        thing.join(".claude/worktrees/pr-5-fix").to_str().unwrap(),
        "made in the clone of other/thing, for wtpr to cd into"
    );
    assert!(
        !here.join(".claude/worktrees").exists(),
        "not in the one you were in"
    );

    let none = w.run(
        &here,
        &[
            "worktree-from-pr",
            "--no-claude",
            "https://github.com/nobody/nothing/pull/5",
        ],
    );
    assert_eq!(none.status.code(), Some(1));
    assert!(stderr(&none).contains("That PR is in nobody/nothing, and no clone of it was found"));
}

#[test]
fn a_forks_pr_opened_trusted_is_on_owner_branch_tracking_the_forks_branch_from_a_remote_named_after_its_owner()
 {
    let (w, _fake) = github_world("wtpr");
    let origin = w.path("widgets.git");
    let fork = w.path("fork.git");
    let clone = w.path("clone");
    w.init(&origin, true);
    w.init(&fork, true);
    std::fs::write(
        w.path(".gitconfig"),
        format!(
            "[url \"{}\"]\n\tinsteadOf = git@github.com:acme/widgets.git\n[url \"{}\"]\n\tinsteadOf = git@github.com:alice/widgets.git\n",
            origin.display(),
            fork.display()
        ),
    )
    .unwrap();
    w.clone_into("git@github.com:acme/widgets.git", &clone);
    w.git(&clone, &["commit", "-q", "--allow-empty", "-m", "start"]);
    w.git(&clone, &["push", "-q", "origin", "main"]);
    // The fork's PR comes from its main, which also names a branch here.
    w.git(
        &clone,
        &["commit", "-q", "--allow-empty", "-m", "alice's fix"],
    );
    let last = w.git(&clone, &["rev-parse", "HEAD"]);
    w.git(
        &clone,
        &[
            "push",
            "-q",
            "git@github.com:alice/widgets.git",
            "HEAD:main",
        ],
    );
    w.git(&clone, &["reset", "-q", "--hard", "HEAD~1"]);

    let cd = w.path("cd");
    w.ok(
        &clone,
        &[
            "worktree-from-pr",
            "--no-claude",
            "--trusted",
            "--cd-file",
            cd.to_str().unwrap(),
            "9",
        ],
    );
    let text = read(&cd);
    let mut lines = text.lines();
    let worktree = PathBuf::from(lines.next().unwrap());
    assert!(
        lines
            .next()
            .unwrap()
            .ends_with("It's alice's PR, so don't push to it unless I ask."),
        "someone else's PR isn't pushed to unasked"
    );
    assert_eq!(
        w.git(&clone, &["config", "remote.alice.url"]),
        "git@github.com:alice/widgets.git",
        "over SSH as origin is"
    );
    assert_eq!(
        w.git(&worktree, &["rev-parse", "HEAD"]),
        last,
        "at the fork's commit"
    );
    assert_eq!(
        w.git(&worktree, &["branch", "--show-current"]),
        "alice-main"
    );
    assert_eq!(
        w.git(&worktree, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
        "alice/main"
    );
}

#[test]
fn a_forks_pr_opens_untrusted_by_default_outside_the_clone_running_no_hook_with_nothing_linked_and_no_claude()
 {
    let (w, _fake) = github_world("wtpr-untrusted");
    let origin = w.path("widgets.git");
    let clone = w.path("clone");
    let ran = w.path("ran");
    w.init(&origin, true);
    std::fs::write(
        w.path(".gitconfig"),
        format!(
            "[url \"{}\"]\n\tinsteadOf = git@github.com:acme/widgets.git\n",
            origin.display()
        ),
    )
    .unwrap();
    w.clone_into("git@github.com:acme/widgets.git", &clone);
    let write_hooks = |whose: &str| {
        for name in ["post-checkout", "post-merge", "post-commit", "pre-commit"] {
            common::stand_in::install(
                &clone.join(".githooks").join(name),
                &format!("#!/bin/sh\necho \"{whose} $0\" >> '{}'\n", ran.display()),
            );
        }
    };
    // Your clone's hooks, committed.
    write_hooks("ours");
    w.git(&clone, &["add", "."]);
    w.git(&clone, &["commit", "-q", "-m", "start"]);
    w.git(&clone, &["push", "-q", "origin", "main"]);
    // The fork's commit, kept by GitHub under refs/pull/9/head, with hooks
    // of its own and a .gitattributes choosing a filter of yours.
    w.git(&clone, &["checkout", "-q", "-b", "theirs"]);
    write_hooks("theirs");
    std::fs::write(clone.join(".gitattributes"), "*.txt filter=mine\n").unwrap();
    std::fs::write(clone.join("f.txt"), "theirs\n").unwrap();
    w.git(&clone, &["add", "."]);
    w.git(&clone, &["commit", "-q", "-m", "their hooks"]);
    let last = w.git(&clone, &["rev-parse", "HEAD"]);
    w.git(&clone, &["push", "-q", "origin", "theirs"]);
    w.git(&origin, &["update-ref", "refs/pull/9/head", &last]);
    w.git(&origin, &["branch", "-q", "-D", "theirs"]);
    w.git(&clone, &["checkout", "-q", "main"]);
    w.git(&clone, &["branch", "-q", "-D", "theirs"]);
    // From now on your clone runs its hooks by their full path, and names a
    // filter.
    let hooks_dir = clone.join(".githooks");
    w.git(
        &clone,
        &["config", "core.hooksPath", hooks_dir.to_str().unwrap()],
    );
    w.git(
        &clone,
        &[
            "config",
            "filter.mine.smudge",
            &format!("echo filter >> '{}'; cat", ran.display()),
        ],
    );
    std::fs::create_dir_all(clone.join(".claude")).unwrap();
    std::fs::write(clone.join(".claude/settings.local.json"), "{}").unwrap();
    let _ = std::fs::remove_file(&ran);

    let cd = w.path("cd");
    let run = |dir: &Path| {
        w.run(
            dir,
            &[
                "worktree-from-pr",
                "--no-claude",
                "--cd-file",
                cd.to_str().unwrap(),
                "9",
            ],
        )
    };
    let out = run(&clone);
    let said = stderr(&out);
    assert_eq!(
        out.status.code(),
        Some(3),
        "which the wtpr function reads as nothing to go into: {said}"
    );
    assert!(read(&cd).is_empty(), "no directory or prompt handed to it");
    let worktree = w.path(".cache/switchboard/pr-review/acme-widgets/9");
    assert_eq!(
        w.git(&worktree, &["rev-parse", "HEAD"]),
        last,
        "at the PR's commit"
    );
    assert_eq!(
        w.git(&worktree, &["branch", "--show-current"]),
        "pr-review/9"
    );
    assert_eq!(
        w.git(&worktree, &["config", "core.hooksPath"]),
        "/dev/null",
        "hooks off for that worktree"
    );
    assert_eq!(
        w.git(&clone, &["config", "core.hooksPath"]),
        hooks_dir.to_str().unwrap(),
        "and still on in your clone"
    );
    assert_eq!(
        w.git(&worktree, &["config", "switchboard.untrusted"]),
        "true"
    );
    assert_eq!(w.git(&worktree, &["config", "core.fsmonitor"]), "false");
    assert_eq!(
        w.git(&worktree, &["config", "submodule.recurse"]),
        "false",
        "nor its submodules fetched from wherever its .gitmodules says"
    );
    assert_eq!(w.git(&worktree, &["config", "filter.mine.smudge"]), "");
    assert!(
        !worktree.join(".claude/settings.local.json").exists(),
        "nothing linked"
    );
    assert_eq!(
        w.git(&clone, &["remote"]),
        "origin",
        "no remote added for the fork"
    );
    assert!(said.contains("PR:       #9 from alice's fork"), "{said}");
    assert!(
        regex::Regex::new(r"Author: {3}alice \(on GitHub \d+ years\)")
            .unwrap()
            .is_match(&said),
        "{said}"
    );
    assert!(said.contains("Changes:  4 files, +4 −4"), "{said}");
    assert!(
        said.contains("git       M .githooks/post-checkout"),
        "pr-risk lists the hooks it changes: {said}"
    );
    assert!(
        !said.contains("Fix"),
        "not the title its author wrote: {said}"
    );
    assert!(said.contains("switchboard codex-review -C"), "{said}");

    // A commit and a merge there run no hook or filter, theirs or yours.
    std::fs::write(worktree.join("g.txt"), "x\n").unwrap();
    w.git(&worktree, &["add", "g.txt"]);
    w.git(&worktree, &["commit", "-q", "-m", "a commit there"]);
    w.git(&worktree, &["merge", "-q", "--no-edit", "--no-ff", "main"]);
    assert!(!ran.exists(), "nothing ran: {}", read(&ran));

    // Run again from another worktree of the same clone, it moves the
    // review worktree to the PR's newest commit.
    w.git(&worktree, &["reset", "-q", "--hard", &last]);
    let other = w.path("other");
    w.git(
        &clone,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "other",
            other.to_str().unwrap(),
        ],
    );
    let _ = std::fs::remove_file(&ran);
    let again = run(&other);
    assert_eq!(again.status.code(), Some(3), "{}", stderr(&again));
    assert!(stderr(&again).contains("moved to the PR's newest commit"));
    assert!(!ran.exists(), "nothing ran: {}", read(&ran));

    // Claude is never started there.
    let here = w.run(&worktree, &["claude-here", "--print"]);
    assert_eq!(here.status.code(), Some(1));
    assert!(stderr(&here).contains("Claude isn't started there"));

    let both = w.run(
        &clone,
        &["worktree-from-pr", "--untrusted", "--trusted", "9"],
    );
    assert_eq!(both.status.code(), Some(1));
    assert!(stderr(&both).contains("can't go together"));

    // Your clone still runs its own hooks, as the review worktree never did.
    w.git(&clone, &["commit", "-q", "--allow-empty", "-m", "mine"]);
    assert!(read(&ran).contains("ours"), "{}", read(&ran));
}

#[test]
fn any_pr_opens_untrusted_when_asked_and_the_wtpr_function_goes_nowhere() {
    let (w, _fake) = github_world("wtpr-asked");
    let origin = w.path("origin.git");
    std::fs::write(
        w.path(".gitconfig"),
        format!(
            "[url \"{}\"]\n\tinsteadOf = git@github.com:o/r.git\n",
            origin.display()
        ),
    )
    .unwrap();
    let clone = w.path("clone");
    w.init(&origin, true);
    w.clone_into("git@github.com:o/r.git", &clone);
    w.git(&clone, &["commit", "-q", "--allow-empty", "-m", "start"]);
    w.git(&clone, &["push", "-q", "origin", "main"]);
    w.git(&clone, &["commit", "-q", "--allow-empty", "-m", "feature"]);
    let last = w.git(&clone, &["rev-parse", "HEAD"]);
    w.git(&clone, &["push", "-q", "origin", "HEAD:feat"]);
    w.git(&origin, &["update-ref", "refs/pull/7/head", &last]);
    w.git(&origin, &["branch", "-q", "-D", "feat"]);
    w.git(&clone, &["reset", "-q", "--hard", "HEAD~1"]);

    // The wtpr function, as a shell runs it: no cd, no Claude, and 0.
    let function =
        String::from_utf8_lossy(&w.run(&clone, &["worktree-from-pr", "init", "bash"]).stdout)
            .into_owned();
    let script = format!("{function}\nwtpr --untrusted 7; echo \"rc=$? pwd=$PWD\"");
    let out = w
        .command("bash")
        .args(["-c", &script])
        .current_dir(&clone)
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains(&format!("rc=0 pwd={}", clone.display())),
        "{said}{}",
        stderr(&out)
    );
    let worktree = w.path(".cache/switchboard/pr-review/o-r/7");
    assert_eq!(w.git(&worktree, &["rev-parse", "HEAD"]), last);
    assert_eq!(
        w.git(&worktree, &["config", "switchboard.untrusted"]),
        "true"
    );
}

#[test]
fn with_the_hooks_path_in_full_a_worktree_runs_the_clones_hooks_never_its_branchs() {
    // Why CLAUDE.md has every clone set core.hooksPath to its .githooks by
    // its full path: a relative one is read from each worktree's top.
    let w = World::new("hooks-path");
    let clone = w.path("clone");
    let ran = w.path("ran");
    w.init(&clone, false);
    let write_hook = |whose: &str| {
        common::stand_in::install(
            &clone.join(".githooks/post-checkout"),
            &format!("#!/bin/sh\necho {whose} >> '{}'\n", ran.display()),
        );
    };
    write_hook("ours");
    w.git(&clone, &["add", "."]);
    w.git(&clone, &["commit", "-q", "-m", "start"]);
    // A branch, as a fork's PR may be, whose own hook would run anything.
    w.git(&clone, &["checkout", "-q", "-b", "theirs"]);
    write_hook("theirs");
    w.git(&clone, &["commit", "-q", "-am", "their hook"]);
    w.git(&clone, &["checkout", "-q", "main"]);

    let hooks = clone.join(".githooks");
    w.git(
        &clone,
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
    let _ = std::fs::remove_file(&ran);
    let full = w.path("full");
    w.git(
        &clone,
        &["worktree", "add", "-q", full.to_str().unwrap(), "theirs"],
    );
    // A checkout in the worktree, as working on the branch there runs one.
    let _ = std::fs::remove_file(&ran);
    w.git(&full, &["checkout", "-q", "-b", "mine"]);
    assert_eq!(
        read(&ran),
        "ours\n",
        "the clone's hook ran, never the branch's"
    );

    // Relative, as it once was: read from the worktree's top, so the hook
    // that runs there is the one its branch carries.
    w.git(&clone, &["config", "core.hooksPath", ".githooks"]);
    let _ = std::fs::remove_file(&ran);
    w.git(&full, &["checkout", "-q", "-b", "mine-too"]);
    assert_eq!(read(&ran), "theirs\n", "the branch's hook ran");
}

// ── worktree-from-ticket ────────────────────────────────────────────────────

fn ticket_answer(body: &Value) -> Value {
    let key = body["variables"]["key"].as_str().unwrap_or("");
    let number = body["variables"]["number"].as_f64().unwrap_or(0.0);
    if key == "ENG" && number == 42.0 {
        return json!({ "data": { "issues": { "nodes": [{ "identifier": "ENG-42", "title": "Fix the login", "branchName": "me/eng-42-fix-the-login", "state": { "name": "Done", "type": "completed" } }] } } });
    }
    json!({ "data": { "issues": { "nodes": [] } } })
}

#[test]
fn a_ticket_gets_a_worktree_on_linears_branch_name_its_prompt_saying_its_closed_state() {
    let mut w = World::new("wt");
    let fake = Fake::linear(ticket_answer);
    w.set("SWITCHBOARD_LINEAR_API", &fake.graphql());
    w.set("LINEAR_API_KEY", "lin_test");
    let repo = w.path("repo");
    w.init(&repo, false);
    w.git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    let cd = w.path("cd");
    w.ok(
        &repo,
        &[
            "worktree-from-ticket",
            "--no-claude",
            "--cd-file",
            cd.to_str().unwrap(),
            "https://linear.app/x/issue/eng-42/fix",
            "--prompt",
            "start\nsmall",
        ],
    );
    let text = read(&cd);
    let mut lines = text.lines();
    let worktree = PathBuf::from(lines.next().unwrap());
    let prompt = lines.next().unwrap();
    assert_eq!(
        worktree,
        repo.join(".claude/worktrees/me/eng-42-fix-the-login")
    );
    assert_eq!(
        w.git(&worktree, &["branch", "--show-current"]),
        "me/eng-42-fix-the-login"
    );
    assert!(
        prompt.starts_with(
            "Work on ticket ENG-42, which is in the \"Done\" state. That state is closed"
        ),
        "{prompt}"
    );
    assert!(
        prompt.ends_with("Also, from me for this ticket: start small"),
        "{prompt}"
    );
    assert_eq!(lines.next(), None, "the prompt is one line");

    let missing = w.run(&repo, &["worktree-from-ticket", "--no-claude", "ENG-7"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(stderr(&missing).contains("no Linear issue found for ENG-7"));
    let usage = w.run(&repo, &["worktree-from-ticket", "not a ticket"]);
    assert_eq!(usage.status.code(), Some(1));
    assert!(stderr(&usage).starts_with("Usage: worktree-from-ticket"));
}

// ── worktree-resume and worktree-done ───────────────────────────────────────

#[test]
fn resume_finds_a_tickets_prs_or_branchs_worktree_offline_and_says_how_to_make_one_that_isnt_there()
{
    let w = World::new("wtr");
    let repo = w.path("repo");
    w.init(&repo, false);
    w.git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    for (dir, branch) in [
        ("abc-12-fix", "abc-12-fix"),
        ("pr-7-feat", "feat"),
        ("abc-123", "abc-123"),
    ] {
        let at = repo.join(".claude/worktrees").join(dir);
        w.git(
            &repo,
            &["worktree", "add", "-q", "-b", branch, at.to_str().unwrap()],
        );
    }
    let cd = w.path("cd");
    for (asked, dir) in [
        ("ABC-12", "abc-12-fix"),
        ("#7", "pr-7-feat"),
        ("feat", "pr-7-feat"),
    ] {
        w.ok(
            &repo,
            &[
                "worktree-resume",
                "--no-claude",
                "--cd-file",
                cd.to_str().unwrap(),
                asked,
            ],
        );
        assert_eq!(
            read(&cd).trim(),
            repo.join(".claude/worktrees").join(dir).to_str().unwrap(),
            "{asked}"
        );
    }
    let none = w.run(&repo, &["worktree-resume", "--no-claude", "ABC-9"]);
    assert_eq!(none.status.code(), Some(1));
    assert!(stderr(&none).contains("no worktree found for ABC-9"));
    assert!(stderr(&none).contains("Create it with:\n  wt ABC-9"));
}

fn done_answer(_: &Value) -> Value {
    json!({ "data": { "issues": { "nodes": [
        { "identifier": "ABC-1", "title": "Shipped one", "url": "u", "state": { "name": "Shipped", "type": "completed" } },
        { "identifier": "ABC-2", "title": "Still going", "url": "u", "state": { "name": "In Progress", "type": "started" } }
    ] } } })
}

#[test]
fn done_lists_the_worktrees_whose_ticket_is_finished_and_removes_nothing_without_a_terminal() {
    let mut w = World::new("wtd");
    let fake = Fake::linear(done_answer);
    w.set("SWITCHBOARD_LINEAR_API", &fake.graphql());
    w.set("LINEAR_API_KEY", "lin_test");
    let repo = w.path("repo");
    w.init(&repo, false);
    w.git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    for (dir, branch) in [
        ("abc-1-ship", "abc-1-ship"),
        ("pr-4-abc-2-go", "abc-2-go"),
        ("loose", "loose"),
    ] {
        let at = repo.join(".claude/worktrees").join(dir);
        w.git(
            &repo,
            &["worktree", "add", "-q", "-b", branch, at.to_str().unwrap()],
        );
    }
    let paths = w.run(&repo, &["worktree-done", "--paths"]);
    assert!(paths.status.success(), "{}", stderr(&paths));
    assert_eq!(
        String::from_utf8_lossy(&paths.stdout).trim(),
        repo.join(".claude/worktrees/abc-1-ship").to_str().unwrap()
    );
    assert!(stderr(&paths).contains("Checking 2 of 3 worktrees against Linear"));
    let listed = w.run(&repo, &["worktree-done", "--all"]);
    let out = String::from_utf8_lossy(&listed.stdout);
    assert!(
        out.contains("Done (1)") && out.contains("ABC-1  Shipped  Shipped one"),
        "{out}"
    );
    assert!(
        out.contains("Still open (1)") && out.contains("No ticket found (1)"),
        "{out}"
    );
    // Without a terminal: a listing, nothing removed.
    let plain = w.run(&repo, &["worktree-done"]);
    assert!(plain.status.success());
    assert!(repo.join(".claude/worktrees/abc-1-ship").exists());
    assert_eq!(
        fake.queries().len(),
        3,
        "one query a run, both tickets in it"
    );
}

// ── worktree-from-note ──────────────────────────────────────────────────────

#[test]
fn an_item_of_a_note_gets_a_worktree_on_note_its_slug_with_the_item_its_plan_and_a_second_run_reusing_it()
 {
    let w = World::new("wtn");
    let repo = w.path("repo");
    let notes = w.path("Notes");
    std::fs::create_dir_all(notes.join("Plans")).unwrap();
    std::fs::write(
        notes.join("Work Notes.md"),
        "# Work Notes\n\n- [ ] Copy Claude's last reply (with a key!)\n  - from the transcript\n\n  Plan: [[Plan - Copy the reply]]\n\n- [ ] something else\n\n- [x] done already\n",
    )
    .unwrap();
    std::fs::write(
        notes.join("Plans/Plan - Copy the reply.md"),
        "# Plan - Copy the reply\n",
    )
    .unwrap();
    w.init(&repo, false);
    w.git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);

    let cd = w.path("cd");
    w.ok(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--cd-file",
            cd.to_str().unwrap(),
            "--note",
            "Work Notes",
            "--item",
            "3",
            "--prompt",
            "start small",
        ],
    );
    let text = read(&cd);
    let (worktree, prompt) = text.trim_end().split_once('\n').unwrap();
    let worktree = PathBuf::from(worktree);
    assert_eq!(
        worktree,
        repo.join(".claude/worktrees/note-copy-claudes-last-reply-with-a-key")
    );
    assert_eq!(
        w.git(&worktree, &["branch", "--show-current"]),
        "note/copy-claudes-last-reply-with-a-key"
    );
    assert!(
        prompt.contains("<item>\n- [ ] Copy Claude's last reply (with a key!)\n  - from the transcript\n\n  Plan: [[Plan - Copy the reply]]\n</item>"),
        "the item and what is indented under it, as written: {prompt}"
    );
    assert!(
        prompt.contains(
            notes
                .join("Plans/Plan - Copy the reply.md")
                .to_str()
                .unwrap()
        ),
        "its plan, found in a folder"
    );
    assert!(
        prompt.contains("compare the item and its plan with the code as it is now")
            && prompt.contains("without building, landing or ticking it"),
        "told to check the item is still worth doing first: {prompt}"
    );
    assert!(prompt.contains("/queue-and-clean"), "told how to land it");
    assert!(
        prompt.contains("as `switchboard landing` says for the repository"),
        "as landing says for the repository"
    );
    assert!(prompt.ends_with("from me for this item: start small"));

    w.ok(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--cd-file",
            cd.to_str().unwrap(),
            "--note",
            "Work Notes.md",
            "--item",
            "3",
        ],
    );
    assert_eq!(
        read(&cd).lines().next().unwrap(),
        worktree.to_str().unwrap(),
        "a second run reuses it"
    );
    // The note got an ID, a blank line and its own line after what was
    // there, so the lines appended below come two further down.
    let note_id = switchboard::notes::id::of_file(&notes.join("Work Notes.md")).unwrap();
    assert_eq!(
        w.git(
            &repo,
            &[
                "config",
                "branch.note/copy-claudes-last-reply-with-a-key.wtnItem"
            ]
        ),
        format!("{note_id}: Copy Claude's last reply (with a key!)"),
        "the branch's claim names the note by its ID"
    );

    let append = |text: &str| {
        let file = notes.join("Work Notes.md");
        let mut all = read(&file);
        all.push_str(text);
        std::fs::write(file, all).unwrap();
    };
    // Another item whose first line slugs the same: refused.
    append("- [ ] Copy Claude's last reply (with a key)?\n");
    let clash = w.run(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--note",
            "Work Notes",
            "--item",
            "13",
        ],
    );
    assert_ne!(clash.status.code(), Some(0));
    assert!(
        stderr(&clash).contains("is for another item"),
        "{}",
        stderr(&clash)
    );
    // The same words in another note: another item.
    std::fs::write(
        notes.join("Other.md"),
        "# Other\n\n- [ ] Copy Claude's last reply (with a key!)\n",
    )
    .unwrap();
    let elsewhere = w.run(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--note",
            "Other",
            "--item",
            "3",
        ],
    );
    assert!(stderr(&elsewhere).contains("is for another item"));
    // A worktree of another branch where this item's would go: refused.
    append("- [ ] Add search\n");
    let taken_at = repo.join(".claude/worktrees/note-add-search");
    w.git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "note-add-search",
            taken_at.to_str().unwrap(),
        ],
    );
    let taken = w.run(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--note",
            "Work Notes",
            "--item",
            "14",
        ],
    );
    assert!(
        stderr(&taken).contains("on branch note-add-search, not note/add-search"),
        "{}",
        stderr(&taken)
    );

    // From another item's worktree with a commit of its own not yet landed.
    w.git(
        &worktree,
        &["commit", "-q", "--allow-empty", "-m", "unlanded"],
    );
    append("- [ ] Third thing\n- [ ] Twice\n- [ ]  Twice  \n");
    w.ok(
        &worktree,
        &[
            "worktree-from-note",
            "--no-claude",
            "--cd-file",
            cd.to_str().unwrap(),
            "--note",
            "Work Notes",
            "--item",
            "15",
        ],
    );
    let third = PathBuf::from(read(&cd).lines().next().unwrap());
    assert_eq!(
        w.git(&third, &["rev-parse", "HEAD"]),
        w.git(&repo, &["rev-parse", "main"]),
        "a new item's branch starts from main, not the worktree it was opened from"
    );
    append("- [ ] Spaced out  \n");
    w.ok(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--note",
            "Work Notes",
            "--item",
            "18",
        ],
    );
    w.ok(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--note",
            "Work Notes",
            "--item",
            "18",
        ],
    );
    let twice = w.run(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--note",
            "Work Notes",
            "--item",
            "16",
        ],
    );
    assert!(stderr(&twice).contains("more than one item reading \"Twice\""));

    let ticked = w.run(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--note",
            "Work Notes",
            "--item",
            "10",
        ],
    );
    assert_ne!(ticked.status.code(), Some(0));
    assert!(stderr(&ticked).contains("ticked already"));
    let indented = w.run(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--note",
            "Work Notes",
            "--item",
            "5",
        ],
    );
    assert_ne!(indented.status.code(), Some(0), "a line that isn't an item");
    let outside = w.run(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--note",
            "../repo/x",
            "--item",
            "1",
        ],
    );
    assert_ne!(
        outside.status.code(),
        Some(0),
        "a file outside the notes directory"
    );
}

#[test]
fn with_an_origin_an_items_branch_starts_from_origins_default_branch_and_an_origin_that_cant_be_fetched_stops_it()
 {
    let w = World::new("wtn-origin");
    let notes = w.path("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("Work.md"),
        "# Work\n\n- [ ] First thing\n- [ ] Second thing\n",
    )
    .unwrap();
    let origin = w.path("origin.git");
    let repo = w.path("repo");
    w.init(&origin, true);
    w.clone_into(origin.to_str().unwrap(), &repo);
    w.git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    w.git(&repo, &["push", "-q", "origin", "main"]);
    let pushed = w.git(&repo, &["rev-parse", "HEAD"]);
    w.git(
        &repo,
        &["commit", "-q", "--allow-empty", "-m", "not pushed"],
    );

    let cd = w.path("cd");
    w.ok(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--cd-file",
            cd.to_str().unwrap(),
            "--note",
            "Work",
            "--item",
            "3",
        ],
    );
    let worktree = PathBuf::from(read(&cd).lines().next().unwrap());
    assert_eq!(
        w.git(&worktree, &["rev-parse", "HEAD"]),
        pushed,
        "from origin's main, not the clone's own"
    );

    std::fs::remove_dir_all(&origin).unwrap();
    let offline = w.run(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--note",
            "Work",
            "--item",
            "4",
        ],
    );
    assert_ne!(offline.status.code(), Some(0));
    assert!(
        stderr(&offline).contains("couldn't fetch origin's"),
        "{}",
        stderr(&offline)
    );
}

#[test]
fn the_items_a_worktree_works_on_are_left_out_until_it_is_removed_and_the_queue_takes_settled_plans_first()
 {
    use std::collections::HashSet;
    use switchboard::cmd::worktree_from_note::{Item, queue, unclaimed};

    let w = World::new("wtn-queue");
    let notes = w.path("Notes");
    std::fs::create_dir_all(notes.join("Work")).unwrap();
    let note = notes.join("Work/Plan's Notes.md");
    std::fs::write(&note, "# Plan's Notes\n\n- [ ] First thing\n- [ ] Second thing\n  Plan: [[Plan - Second]] (needs a choice)\n- [ ] Third thing\n").unwrap();
    std::fs::write(notes.join("Other.md"), "# Other\n\n- [ ] Third thing\n").unwrap();
    let repo = w.path("repo");
    w.init(&repo, false);
    w.git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    w.ok(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--note",
            "Work/Plan's Notes",
            "--item",
            "4",
        ],
    );
    w.ok(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--note",
            "Other",
            "--item",
            "3",
        ],
    );
    let rows = |items: Vec<Item>| items.iter().map(Item::row).collect::<Vec<_>>();
    let listed = || rows(unclaimed(&repo, "Work/Plan's Notes.md", &read(&note)));
    assert_eq!(
        listed(),
        vec!["3\tFirst thing", "6\tThird thing"],
        "the item with a worktree left out, a like item of another note kept"
    );
    w.git(
        &repo,
        &[
            "worktree",
            "remove",
            repo.join(".claude/worktrees/note-second-thing")
                .to_str()
                .unwrap(),
        ],
    );
    assert_eq!(
        listed(),
        vec![
            "3\tFirst thing",
            "4\tSecond thing (needs a choice)",
            "6\tThird thing"
        ],
        "back once its worktree is removed, its branch kept"
    );

    // The queue autopilot takes.
    std::fs::write(
        notes.join("Work.md"),
        [
            "# Work",
            "",
            "- [ ] Plain",
            "- [ ] ",
            "- [ ] Planned",
            "  Plan: [[Plan - Planned]]",
            "- [ ] Undecided",
            "  Plan: [[Plan - Undecided]] (needs a choice)",
            "- [ ] Taken",
            "  Plan: [[Plan - Taken]]",
            "- [ ] Also planned",
            "  Plan: [[Plan - Also]]",
            "- [ ] Also plain",
            "- [ ] Open in its plan",
            "  Plan: [[Plan - Open]]",
            "- [ ] Lost plan",
            "  Plan: [[Plan - Gone]]",
            "",
        ]
        .join("\n"),
    )
    .unwrap();
    std::fs::create_dir_all(notes.join("Plans")).unwrap();
    for name in ["Planned", "Also", "Taken"] {
        std::fs::write(
            notes.join(format!("Plans/Plan - {name}.md")),
            format!("# Plan - {name}\n\nChosen: the first.\n\n## Steps\n"),
        )
        .unwrap();
    }
    std::fs::write(
        notes.join("Plans/Plan - Open.md"),
        "# Plan - Open\n\n## Decision needed\n\nA or B.\n",
    )
    .unwrap();
    w.ok(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--note",
            "Work",
            "--item",
            "9",
        ],
    );
    let taken = |skip: &[&str], ready| {
        let skip: HashSet<String> = skip.iter().map(|s| s.to_string()).collect();
        let claims = w.path("state/wtn-claims.tsv");
        rows(queue(&notes, &repo, "Work.md", &skip, ready, false, &claims).unwrap())
    };
    assert_eq!(
        taken(&[], false),
        vec![
            "5\tPlanned",
            "11\tAlso planned",
            "3\tPlain",
            "13\tAlso plain",
            "16\tLost plan"
        ],
        "a plan whose note isn't there taken as no plan, one whose note needs a decision never, and an empty item never"
    );
    assert_eq!(
        taken(&[], true),
        vec!["5\tPlanned", "11\tAlso planned"],
        "only items with a settled plan"
    );
    assert_eq!(
        taken(&["Planned", "Plain"], false),
        vec!["11\tAlso planned", "13\tAlso plain", "16\tLost plan"],
        "the skipped ones left out"
    );

    // An item wtn started is taken for half an hour, even once its worktree
    // and branch are gone, since queue-and-clean may remove them a moment
    // before it ticks the item.
    w.ok(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--note",
            "Work",
            "--item",
            "3",
        ],
    );
    let claims = read(&w.path("state/wtn-claims.tsv"));
    let work_id = switchboard::notes::id::of_file(&notes.join("Work.md")).unwrap();
    assert!(
        claims.contains(&format!("\t{work_id}\tPlain\n")),
        "each by its note's ID: {claims}"
    );
    let tree = repo.join(".claude/worktrees/note-plain");
    w.git(&repo, &["worktree", "remove", tree.to_str().unwrap()]);
    w.git(&repo, &["branch", "-D", "note/plain"]);
    assert!(
        !taken(&[], false).contains(&"3\tPlain".to_string()),
        "claimed lately, so still taken"
    );
}

// test/worktree-from-note.test.js's "a claim names its note by ID, so a
// note renamed with mv keeps its item out of the picker and wtn reuses the
// claim; claims from before IDs are brought to them once, by path or by
// title".
#[test]
fn a_claim_names_its_note_by_id_so_a_renamed_notes_item_stays_taken_and_older_claims_are_brought_to_ids_once()
 {
    use switchboard::cmd::worktree_from_note::{Item, unclaimed};
    use switchboard::notes::id;

    let w = World::new("wtn-ids");
    let notes = w.path("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("Work.md"),
        "# Work\n\n- [ ] Add search\n- [ ] Other\n",
    )
    .unwrap();
    let repo = w.path("repo");
    w.init(&repo, false);
    w.git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    let config = |key: &str| {
        String::from_utf8_lossy(
            &w.command("git")
                .args(["-C", repo.to_str().unwrap(), "config", key])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string()
    };
    let wtn = |note: &str, item: &str| {
        w.run(
            &repo,
            &[
                "worktree-from-note",
                "--no-claude",
                "--note",
                note,
                "--item",
                item,
            ],
        )
    };

    assert!(wtn("Work", "3").status.success());
    let work_id = id::of_file(&notes.join("Work.md")).unwrap();
    assert_eq!(
        config("branch.note/add-search.wtnItem"),
        format!("{work_id}: Add search")
    );

    std::fs::create_dir_all(notes.join("Projects")).unwrap();
    let renamed = notes.join("Projects/Renamed.md");
    std::fs::rename(notes.join("Work.md"), &renamed).unwrap();
    let rows = || {
        unclaimed(&repo, "Projects/Renamed.md", &read(&renamed))
            .iter()
            .map(Item::row)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        rows(),
        ["4\tOther"],
        "renamed outside Notes, its item is still taken"
    );
    std::fs::write(&renamed, read(&renamed).replace("-->\n", "-->  \n")).unwrap();
    assert_eq!(
        rows(),
        ["4\tOther"],
        "and with spaces an editor left after its ID line"
    );
    std::fs::write(&renamed, read(&renamed).replace("-->  \n", "-->\n")).unwrap();
    let again = wtn("Projects/Renamed", "3");
    assert!(
        again.status.success(),
        "wtn on it reuses the claim rather than failing: {}",
        stderr(&again)
    );
    assert!(stderr(&again).contains("reusing it"), "{}", stderr(&again));

    // Claims from before notes had IDs, by the note's path: one still
    // there, one whose note was renamed since (found by its title), one lost.
    std::fs::write(notes.join("Kept.md"), "# Kept\n\n- [ ] Kept item\n").unwrap();
    std::fs::write(notes.join("Moved.md"), "# Gone\n\n- [ ] Moved item\n").unwrap();
    w.git(&repo, &["config", "--unset", "switchboard.wtnClaimsById"]);
    w.git(
        &repo,
        &[
            "config",
            "branch.note/kept-item.wtnItem",
            "Kept.md: Kept item",
        ],
    );
    w.git(
        &repo,
        &[
            "config",
            "branch.note/moved-item.wtnItem",
            "Gone.md: Moved item",
        ],
    );
    w.git(
        &repo,
        &[
            "config",
            "branch.note/lost-item.wtnItem",
            "Lost.md: Lost item",
        ],
    );
    // A note renamed, and an unrelated one made under its old name since.
    std::fs::write(
        notes.join("Old name.md"),
        "# Unrelated\n\n- [ ] Something else\n",
    )
    .unwrap();
    std::fs::write(
        notes.join("New name.md"),
        "# New name\n\n- [ ] Renamed item\n",
    )
    .unwrap();
    w.git(
        &repo,
        &[
            "config",
            "branch.note/renamed-item.wtnItem",
            "Old name.md: Renamed item",
        ],
    );
    std::fs::write(notes.join("More.md"), "# More\n\n- [ ] Something\n").unwrap();
    let run = wtn("More", "3");
    assert!(run.status.success(), "{}", stderr(&run));
    let id_of = |name: &str| id::of_file(&notes.join(name)).unwrap();
    assert_eq!(
        config("branch.note/kept-item.wtnItem"),
        format!("{}: Kept item", id_of("Kept.md")),
        "by its path"
    );
    assert_eq!(
        config("branch.note/moved-item.wtnItem"),
        format!("{}: Moved item", id_of("Moved.md")),
        "by its title after a rename"
    );
    assert_eq!(
        config("branch.note/lost-item.wtnItem"),
        "Lost.md: Lost item",
        "one it can't place left as it was"
    );
    assert_eq!(
        config("branch.note/renamed-item.wtnItem"),
        format!("{}: Renamed item", id_of("New name.md")),
        "not the unrelated note at its old path, but the one holding its item"
    );
    assert!(stderr(&run).contains("Lost.md: Lost item"), "and listed");
    let later = wtn("More", "3");
    assert!(!stderr(&later).contains("Lost.md"), "once");
    // The lost note back at its path: wtn takes the old claim as its item's
    // and brings it to the ID.
    std::fs::write(notes.join("Lost.md"), "# Lost\n\n- [ ] Lost item\n").unwrap();
    let back = wtn("Lost", "3");
    assert!(
        back.status.success(),
        "an old claim by path is still its item's: {}",
        stderr(&back)
    );
    assert_eq!(
        config("branch.note/lost-item.wtnItem"),
        format!("{}: Lost item", id_of("Lost.md")),
        "and brought to the ID"
    );

    // Another item whose branch is taken: named by its words and its
    // note's title, wherever it is now.
    let mut more = read(&notes.join("More.md"));
    more.push_str("- [ ] Add search!\n");
    std::fs::write(notes.join("More.md"), more).unwrap();
    let line = read(&notes.join("More.md"))
        .lines()
        .position(|l| l == "- [ ] Add search!")
        .unwrap()
        + 1;
    let clash = wtn("More", &line.to_string());
    assert!(
        stderr(&clash).contains("is for another item: \"Add search\" in Work."),
        "{}",
        stderr(&clash)
    );
}

// test/worktree-from-note.test.js's "several wtn started at once on old
// path-form claims bring them to IDs once, none of them failing", here
// several threads at once.
#[test]
fn several_bringing_old_claims_to_ids_at_once_do_it_once_and_none_fails() {
    let w = World::new("wtn-migrate-race");
    let notes = w.path("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    let repo = w.path("repo");
    w.init(&repo, false);
    for n in 0..8 {
        std::fs::write(
            notes.join(format!("N{n}.md")),
            format!("# N{n}\n\n- [ ] item {n}\n"),
        )
        .unwrap();
        w.git(
            &repo,
            &[
                "config",
                &format!("branch.note/item-{n}.wtnItem"),
                &format!("N{n}.md: item {n}"),
            ],
        );
    }
    let unplaced: Vec<Vec<String>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..5)
            .map(|_| s.spawn(|| switchboard::notes::id::migrate_claims(&repo, &notes)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(unplaced.iter().all(Vec::is_empty), "{unplaced:?}");
    for n in 0..8 {
        let note_id = switchboard::notes::id::of_file(&notes.join(format!("N{n}.md"))).unwrap();
        assert_eq!(
            w.git(&repo, &["config", &format!("branch.note/item-{n}.wtnItem")]),
            format!("{note_id}: item {n}")
        );
    }
}

#[test]
fn wt_s_claude_function_takes_the_plugin_as_the_setting_says_at_each_launch() {
    let dir = TempDir::new("sbw-claude");
    let home = dir.path().join("home");
    let bin = home.join("bin");
    // A stand-in claude saying the plugin folders it was started with.
    common::stand_in::Claude::new()
        .answer("echo \"dirs=${CLAUDE_CODE_PLUGIN_DIRS-unset}\"")
        .install(&bin);
    let plugin = home.join(".local/share/switchboard/plugin");
    let ours = plugin.to_string_lossy().into_owned();
    let settings = dir.path().join("settings.json");
    // A shell's value from before the setting changed, with a folder of
    // the person's own beside Switchboard's.
    let run = |on: bool, in_workspace: bool, before: &str| {
        std::fs::write(&settings, json!({ "claude_plugin": on }).to_string()).unwrap();
        let mut cmd = Command::new("bash");
        cmd.arg("-c")
            .arg(format!(
                "eval \"$('{}' worktree-from-ticket init bash)\"; claude",
                BIN
            ))
            .env("HOME", &home)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("SWITCHBOARD_SETTINGS", &settings)
            .env("CLAUDE_CODE_PLUGIN_DIRS", before)
            .env_remove("SWITCHBOARD_PANE")
            .env_remove("SWITCHBOARD_STATE_DIR");
        if in_workspace {
            cmd.env("SWITCHBOARD", dir.path().join("sock"));
        } else {
            cmd.env_remove("SWITCHBOARD");
        }
        let out = cmd.output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let with = format!("/opt/mine:{ours}");
    assert_eq!(run(false, true, &with), "dirs=/opt/mine", "off: taken out");
    assert_eq!(
        run(false, true, &ours),
        "dirs=unset",
        "off, nothing else: unset"
    );
    assert_eq!(
        run(true, true, "/opt/mine"),
        format!("dirs={with}"),
        "on: put back"
    );
    assert_eq!(
        run(false, false, &with),
        format!("dirs={with}"),
        "outside the workspace, left as it is"
    );
    common::stand_in::assert_none_refused(&bin);
}

// Autopilot's target: `wtn --target` cuts the item's branch from that
// branch rather than the default one, a branch only origin has made a local
// one tracking it, and records it, which `switchboard landing` names with
// the remote branch it tracks, or none for a local-only one.
#[test]
fn an_items_branch_is_cut_from_its_target_which_landing_names_with_what_it_tracks() {
    let w = World::new("wtn-target");
    let notes = w.path("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("Work.md"),
        "# Work\n\n- [ ] First thing\n- [ ] Second thing\n- [ ] Third thing\n",
    )
    .unwrap();
    let (origin, repo) = w.origin_and_clone();
    let _ = origin;
    // A local-only branch a commit past main.
    w.git(&repo, &["branch", "local-dev"]);
    let dev = w.path("dev-wt");
    w.git(
        &repo,
        &["worktree", "add", "-q", dev.to_str().unwrap(), "local-dev"],
    );
    w.git(&dev, &["commit", "-q", "--allow-empty", "-m", "dev only"]);
    let dev_tip = w.git(&dev, &["rev-parse", "HEAD"]);
    // A branch on origin alone.
    w.git(&dev, &["push", "-q", "origin", "local-dev:shared"]);

    let wtn = |item: &str, target: &str| {
        let cd = w.path("cd");
        w.ok(
            &repo,
            &[
                "worktree-from-note",
                "--no-claude",
                "--cd-file",
                cd.to_str().unwrap(),
                "--note",
                "Work",
                "--item",
                item,
                "--target",
                target,
            ],
        );
        PathBuf::from(read(&cd).lines().next().unwrap())
    };
    let landing_target = |dir: &Path| -> serde_json::Value {
        let out = w.run(dir, &["landing", "--json"]);
        assert!(out.status.success(), "{}", stderr(&out));
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["target"].clone()
    };

    let first = wtn("3", "local-dev");
    assert_eq!(w.git(&first, &["rev-parse", "HEAD"]), dev_tip);
    assert_eq!(
        w.git(&repo, &["config", "branch.note/first-thing.wtnTarget"]),
        "local-dev"
    );
    assert_eq!(
        landing_target(&first),
        serde_json::json!({ "branch": "local-dev", "remote": null, "remoteBranch": null })
    );

    let second = wtn("4", "shared");
    assert_eq!(w.git(&second, &["rev-parse", "HEAD"]), dev_tip);
    assert_eq!(
        landing_target(&second),
        serde_json::json!({ "branch": "shared", "remote": "origin", "remoteBranch": "shared" })
    );

    // None given: the default branch, and no target for landing.
    let cd = w.path("cd");
    w.ok(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--cd-file",
            cd.to_str().unwrap(),
            "--note",
            "Work",
            "--item",
            "5",
        ],
    );
    let third = PathBuf::from(read(&cd).lines().next().unwrap());
    assert_eq!(landing_target(&third), serde_json::Value::Null);

    let missing = w.run(
        &repo,
        &[
            "worktree-from-note",
            "--no-claude",
            "--note",
            "Work",
            "--item",
            "5",
            "--target",
            "nowhere",
        ],
    );
    assert!(
        stderr(&missing).contains("no branch nowhere"),
        "{}",
        stderr(&missing)
    );
}
