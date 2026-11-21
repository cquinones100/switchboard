//! What a new tab opens beside its narrow lone pane (Settings'
//! `tab_opens_beside`): the first it has of its PR, its ticket and its
//! plan or note, the focus left in its pane, a PR found later taking the
//! place of what comes after it, and nothing once you have closed it.

#[cfg(feature = "net")]
mod beside {
    use std::path::Path;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};

    use crate::common::http::{Fake, Reply};
    use crate::common::{self, TestServer};

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    /// GitHub with issue 12 on acme/widgets, and PR 41 on its branch when
    /// `pr`.
    fn github(pr: bool) -> Fake {
        Fake::github("test-token", move |request| {
            let path = request.path();
            if path.starts_with("/repos/acme/widgets/issues/12/comments") {
                return Some(Reply::json(200, json!([])));
            }
            if path.starts_with("/repos/acme/widgets/issues/12") {
                return Some(Reply::json(
                    200,
                    json!({ "number": 12, "title": "Make the widget blue", "state": "open",
                        "body": "It is red.", "user": { "login": "sam" },
                        "created_at": "2026-10-01T09:00:00Z", "updated_at": "2026-10-01T09:00:00Z",
                        "assignees": [], "labels": [], "comments": 0 }),
                ));
            }
            if path != "/graphql" {
                return None;
            }
            let node = json!({
                "number": 41, "url": "https://github.com/acme/widgets/pull/41",
                "title": "Blue widgets", "state": "OPEN", "isDraft": false,
                "baseRefName": "main", "headRefName": "issue/12-make-it-blue",
                "headRepositoryOwner": {"login": "acme"}, "author": {"login": "jo"},
                "commits": {"totalCount": 1}, "changedFiles": 0, "body": "Why.",
                "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN", "mergeQueueEntry": null
            });
            let nodes = if pr { vec![node.clone()] } else { vec![] };
            let answer = if request.text().contains("pullRequests") {
                json!({"data": {"repository": {"pullRequests": {"nodes": nodes}}}})
            } else {
                json!({"data": {"repository": {"pullRequest": if pr { node } else { Value::Null }}}})
            };
            Some(Reply::json(200, answer))
        })
    }

    /// A server asking `fake` for GitHub, its lone panes capped at 80
    /// columns, with `extra` settings over these.
    fn server(fake: &Fake, extra: Value) -> TestServer {
        let mut settings = json!({"notifications": false, "tips": false, "linear": false,
            "sidebar": false, "photos": false, "lone_pane_width": "80",
            "tab_opens_beside": true});
        for (k, v) in extra.as_object().unwrap() {
            settings[k] = v.clone();
        }
        let server = TestServer::start_with_env_and_settings(
            &[
                ("GH_TOKEN", "test-token"),
                ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
                ("SWITCHBOARD_GITHUB_API", &fake.url),
            ],
            &settings.to_string(),
        );
        let config = server.home.join(".config/switchboard");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("repos.json"),
            json!({ "acme/widgets": { "tickets": "github" } }).to_string(),
        )
        .unwrap();
        server
    }

    /// acme/widgets checked out on issue 12's branch.
    fn widgets(server: &TestServer) -> std::path::PathBuf {
        let work = server.dir.path().join("widgets");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q"]);
        git(
            &work,
            &["symbolic-ref", "HEAD", "refs/heads/issue/12-make-it-blue"],
        );
        git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
        );
        work
    }

    /// A plain folder with no repository.
    fn plain(server: &TestServer) -> std::path::PathBuf {
        let dir = server.dir.path().join("plain");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The second tab's panes, the one the test opened.
    fn panes(server: &TestServer) -> Vec<Value> {
        server.list()["sessions"][0]["tabs"][1]["panes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    fn wait_until(server: &TestServer, what: &str, done: &dyn Fn(&[Value]) -> bool) -> Vec<Value> {
        let deadline = common::deadline(Duration::from_secs(10));
        loop {
            let now = panes(server);
            if done(&now) {
                return now;
            }
            assert!(Instant::now() < deadline, "waiting for {what}: {now:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Nothing has opened beside the tab's pane a while after it opened.
    fn stays_alone(server: &TestServer) {
        let until = Instant::now() + Duration::from_millis(1500) * common::patience();
        while Instant::now() < until {
            let now = panes(server);
            assert_eq!(now.len(), 1, "nothing opened beside it: {now:?}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// The tab named by its PR, once its lookup has answered.
    fn wait_for_pr(server: &TestServer) {
        let deadline = common::deadline(Duration::from_secs(10));
        loop {
            let label = server.list()["sessions"][0]["tabs"][1]["label"].clone();
            if label.as_str().is_some_and(|l| l.contains("#41")) {
                return;
            }
            assert!(Instant::now() < deadline, "waiting for the PR: {label}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn marked(pane: &Value, mark: &str) -> bool {
        pane["options"][mark] == "1"
    }

    /// Opens a tab in `dir`, in front, and gives its pane's id.
    fn new_tab(server: &TestServer, dir: &Path) -> String {
        server.ok(&["new-tab", "-c", &dir.to_string_lossy()]);
        wait_until(server, "the tab", &|p| !p.is_empty())[0]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn a_tab_with_a_pr_and_a_ticket_opens_the_pr_view_beside_its_pane() {
        let fake = github(true);
        let server = server(&fake, json!({}));
        let work = widgets(&server);
        let mut client = server.attach(240, 36);
        client.wait_text("$");
        let first = new_tab(&server, &work);

        let shown = wait_until(&server, "the PR beside", &|p| {
            p.len() == 2 && marked(&p[1], "@pr_view")
        });
        assert_eq!(shown[0]["id"], first.as_str(), "{shown:?}");
        assert_eq!(
            shown[0]["active"], true,
            "the keys stay in the pane: {shown:?}"
        );
        assert!(
            !shown.iter().any(|p| marked(p, "@ticket_view")),
            "{shown:?}"
        );
    }

    #[test]
    fn a_tab_with_only_a_ticket_opens_the_ticket_view() {
        let fake = github(false);
        let server = server(&fake, json!({}));
        let work = widgets(&server);
        let mut client = server.attach(240, 36);
        client.wait_text("$");
        new_tab(&server, &work);

        let shown = wait_until(&server, "the ticket beside", &|p| {
            p.len() == 2 && marked(&p[1], "@ticket_view")
        });
        client.wait_text("It is red.");
        assert_eq!(shown[0]["active"], true, "{shown:?}");
    }

    #[test]
    fn a_tab_with_a_note_opens_it_and_a_pr_found_later_takes_its_place() {
        let fake = github(true);
        let server = server(&fake, json!({}));
        let work = widgets(&server);
        let notes = server.home.join("Notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(
            notes.join("Blue plan.md"),
            "# Blue plan\n\n- [ ] Paint it\n",
        )
        .unwrap();
        let mut client = server.attach(240, 36);
        client.wait_text("$");
        let first = new_tab(&server, &plain(&server));
        server.ok(&["set", "-t", &first, "@tab_note_by_you", "Blue plan.md"]);

        wait_until(&server, "Notes beside", &|p| {
            p.len() == 2 && marked(&p[1], "@notes_pane")
        });
        client.wait_text("Paint it");

        // The pane, which kept the keys, moves to the PR's branch.
        client.wait_text("$");
        client.type_text(&format!("cd {}\r", work.display()));
        let shown = wait_until(&server, "the PR in Notes' place", &|p| {
            p.len() == 2 && marked(&p[1], "@pr_view")
        });
        assert_eq!(shown[0]["id"], first.as_str(), "{shown:?}");
        assert_eq!(shown[0]["active"], true, "{shown:?}");
    }

    #[test]
    fn a_tab_with_nothing_of_its_own_opens_nothing() {
        let fake = github(false);
        let server = server(&fake, json!({}));
        let mut client = server.attach(240, 36);
        client.wait_text("$");
        new_tab(&server, &plain(&server));
        stays_alone(&server);
    }

    #[test]
    fn a_view_closed_by_hand_isnt_opened_again() {
        let fake = github(false);
        let server = server(&fake, json!({}));
        let work = widgets(&server);
        let notes = server.home.join("Notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(notes.join("Blue plan.md"), "# Blue plan\n").unwrap();
        let mut client = server.attach(240, 36);
        client.wait_text("$");
        let first = new_tab(&server, &work);
        wait_until(&server, "the ticket beside", &|p| {
            p.len() == 2 && marked(&p[1], "@ticket_view")
        });
        client.wait_text("It is red.");

        // M-i closes it; a note set since opens nothing.
        client.send(b"\x1bi");
        wait_until(&server, "the ticket closed", &|p| p.len() == 1);
        server.ok(&["set", "-t", &first, "@tab_note_by_you", "Blue plan.md"]);
        stays_alone(&server);
    }

    #[test]
    fn nothing_opens_with_the_setting_off_or_no_room() {
        let fake = github(true);
        let server = server(&fake, json!({"tab_opens_beside": false}));
        let work = widgets(&server);
        let mut client = server.attach(240, 36);
        client.wait_text("$");
        new_tab(&server, &work);
        wait_for_pr(&server);
        stays_alone(&server);

        let fake = github(true);
        let server = self::server(&fake, json!({"lone_pane_width": "off"}));
        let work = widgets(&server);
        let mut client = server.attach(240, 36);
        client.wait_text("$");
        new_tab(&server, &work);
        wait_for_pr(&server);
        stays_alone(&server);
    }

    #[test]
    fn a_tab_opened_behind_stays_behind_as_its_view_opens() {
        let fake = github(true);
        let server = server(&fake, json!({}));
        let work = widgets(&server);
        let mut client = server.attach(240, 36);
        client.wait_text("$");
        server.ok(&["new-tab", "-d", "-c", &work.to_string_lossy()]);

        let shown = wait_until(&server, "the PR beside", &|p| {
            p.len() == 2 && marked(&p[1], "@pr_view")
        });
        assert_eq!(shown[0]["active"], true, "{shown:?}");
        let tabs = server.list()["sessions"][0]["tabs"].clone();
        assert_eq!(
            tabs[0]["current"], true,
            "the first tab stays in front: {tabs}"
        );
        assert_eq!(tabs[1]["current"], false, "{tabs}");
    }
}
