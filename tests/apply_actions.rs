//! `switchboard apply-actions`, which lists what a landed change needs to
//! take effect, run as queue-and-clean runs it, on throwaway repositories
//! with a temporary HOME, so nothing reads your own repos.json or
//! trusted-dirs. Ported from test/apply-actions.test.js; the tmux version's
//! apply.sh tests went with tmux.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::{BIN, TempDir};

struct Repo {
    _tmp: TempDir,
    home: PathBuf,
    repo: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

impl Repo {
    fn new() -> Repo {
        let tmp = TempDir::new("apply");
        let home = tmp.path().join("home");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        Repo {
            _tmp: tmp,
            home,
            repo,
        }
    }

    /// Commits the files given, returning the commit.
    fn commit(&self, files: &[(&str, &str)]) -> String {
        for (file, text) in files {
            let path = self.repo.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        git(&self.repo, &["add", "-A"]);
        git(&self.repo, &["commit", "-q", "-m", "change"]);
        git(&self.repo, &["rev-parse", "HEAD"])
    }

    /// The actions for `from..HEAD`, as { key: [label, why, command] }.
    fn actions(&self, from: &str) -> BTreeMap<String, Vec<String>> {
        self.lines(from)
            .into_iter()
            .map(|line| {
                let mut parts = line.split('\t').map(String::from);
                (parts.next().unwrap(), parts.collect())
            })
            .collect()
    }

    /// The keys for `from..HEAD`, in the order listed.
    fn keys(&self, from: &str) -> Vec<String> {
        self.lines(from)
            .iter()
            .map(|l| l.split('\t').next().unwrap().to_string())
            .collect()
    }

    fn lines(&self, from: &str) -> Vec<String> {
        let out = Command::new(BIN)
            .args(["apply-actions", self.repo.to_str().unwrap(), from, "HEAD"])
            .env("HOME", &self.home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env_remove("SWITCHBOARD")
            .env_remove("SWITCHBOARD_PANE")
            .env_remove("SWITCHBOARD_STATE_DIR")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(String::from)
            .collect()
    }

    fn own_file(&self, name: &str, text: &str) {
        let dir = self.home.join(".config/switchboard");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), text).unwrap();
    }
}

#[test]
fn in_switchboard_a_change_to_the_code_is_built_and_the_workspace_restarted_once_each_and_docs_need_nothing()
 {
    let r = Repo::new();
    // Switchboard is ~/switchboard, by its real path.
    std::os::unix::fs::symlink(&r.repo, r.home.join("switchboard")).unwrap();
    let start = r.commit(&[("Cargo.toml", "[package]\n"), ("README.md", "a")]);
    r.commit(&[
        ("src/sidebar/model.rs", "1"),
        ("src/sidebar/render.rs", "2"),
        ("assets/settings.json", "[]"),
    ]);
    let listed = r.actions(&start);
    assert_eq!(r.keys(&start), ["install", "restart"]);
    assert_eq!(
        listed["install"][0],
        "Build Switchboard and put it in place of the one running (cargo build --release)"
    );
    assert_eq!(
        listed["install"][1],
        "assets/settings.json and 2 more changed"
    );
    assert_eq!(
        listed["install"][2],
        format!("switchboard apply install '{}'", r.repo.display())
    );
    assert_eq!(
        listed["restart"][2], "",
        "restarting the workspace is yours"
    );

    let before = r.commit(&[("README.md", "b")]);
    r.commit(&[
        ("README.md", "c"),
        ("docs/guide.md", "d"),
        ("tests/x.rs", "e"),
    ]);
    assert!(r.actions(&before).is_empty(), "docs and tests need nothing");
}

#[test]
fn a_repository_shaped_like_switchboard_that_isnt_your_clone_gets_none_of_its_list() {
    let r = Repo::new();
    let start = r.commit(&[("Cargo.toml", "[package]\n")]);
    r.commit(&[("src/main.rs", "fn main() {}")]);
    assert!(r.actions(&start).is_empty());
}

#[test]
fn elsewhere_a_changed_lockfile_installs_where_packages_are_new_migrations_run_and_env_example_is_yours_the_scripts_only_in_a_trusted_clone()
 {
    let r = Repo::new();
    let start = r.commit(&[
        ("package.json", r#"{"scripts":{"db:migrate":"x"}}"#),
        ("pnpm-lock.yaml", "a"),
        ("web/package-lock.json", "a"),
        ("pnp/yarn.lock", "a"),
        ("pnp/.pnp.cjs", "x"),
    ]);
    std::fs::create_dir_all(r.repo.join("node_modules")).unwrap();
    r.commit(&[
        ("pnpm-lock.yaml", "b"),
        ("web/package-lock.json", "b"),
        ("pnp/yarn.lock", "b"),
        ("db/migrations/001_add.sql", "x"),
        (".env.example", "A=1"),
    ]);
    let untrusted = r.actions(&start);
    assert_eq!(
        untrusted["install:."][2], "",
        "a clone not in trusted-dirs runs none of its scripts"
    );
    assert!(
        untrusted["install:."][0].ends_with("yourself (the clone isn't in trusted-dirs)"),
        "{:?}",
        untrusted["install:."]
    );
    assert_eq!(untrusted["migrate:."][2], "");

    r.own_file("trusted-dirs", &format!("# mine\n{}\n", r.repo.display()));
    let listed = r.actions(&start);
    assert_eq!(
        r.keys(&start),
        ["install:pnp", "install:.", "migrate:.", "env"],
        "web/ has nothing installed, so nothing there is out of date, and Yarn's Plug'n'Play counts as installed"
    );
    assert_eq!(listed["install:."][0], "Install packages with pnpm");
    assert_eq!(
        listed["install:."][2],
        format!(
            "cd '{}' && pnpm install --frozen-lockfile",
            r.repo.display()
        )
    );
    assert_eq!(
        listed["migrate:."][2],
        format!("cd '{}' && pnpm run db:migrate", r.repo.display())
    );
    assert_eq!(listed["env"][2], "");
}

#[test]
fn a_repos_json_apply_list_replaces_the_defaults_listed_for_its_paths_and_run_in_a_trusted_clone() {
    let r = Repo::new();
    git(
        &r.repo,
        &["remote", "add", "origin", "git@github.com:someone/app.git"],
    );
    r.own_file(
        "repos.json",
        r#"{ "someone/app": { "apply": [
            { "paths": ["server/**"], "label": "Restart the dev server", "command": "make restart" },
            { "paths": ["docs/*.md"], "label": "Rebuild the docs", "command": "make docs" },
            { "paths": ["server/**/config.js"], "label": "Reload the config", "command": "make config" }
        ] } }"#,
    );
    let start = r.commit(&[("server/a.js", "1"), ("pnpm-lock.yaml", "a")]);
    std::fs::create_dir_all(r.repo.join("node_modules")).unwrap();
    r.commit(&[
        ("server/deep/b.js", "2"),
        ("server/not-config.js", "x"),
        ("pnpm-lock.yaml", "b"),
    ]);
    let listed = r.actions(&start);
    assert_eq!(
        r.keys(&start),
        ["own:Restart the dev server"],
        "it may run the clone's Makefile"
    );
    assert_eq!(
        listed["own:Restart the dev server"],
        [
            "Restart the dev server yourself (the clone isn't in trusted-dirs)",
            "server/deep/b.js and 1 more changed",
            ""
        ]
    );
    r.own_file("trusted-dirs", &format!("{}\n", r.repo.display()));
    assert_eq!(
        r.actions(&start)["own:Restart the dev server"],
        [
            "Restart the dev server".to_string(),
            "server/deep/b.js and 1 more changed".to_string(),
            format!("cd '{}' && make restart", r.repo.display())
        ]
    );
}

#[test]
fn apply_builds_only_your_own_clone_or_a_trusted_one() {
    let r = Repo::new();
    let out = Command::new(BIN)
        .args(["apply", "install", r.repo.to_str().unwrap()])
        .env("HOME", &r.home)
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("trusted-dirs"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = Command::new(BIN)
        .args(["apply", "relay"])
        .env("HOME", &r.home)
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .env_remove("SWITCHBOARD_STATE_DIR")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("isn't running"));
}
