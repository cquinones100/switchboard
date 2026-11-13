//! Your day as the server keeps it, end to end in a temporary HOME: the
//! meeting notes from Fathom, the standup and today's to-do list written
//! into a notes directory of the test's own, the Slack look sorting what a
//! search found, and the day /today saves read back as the Calendar view.
//! Fathom, Slack and Claude are stand-ins that answer as they would, so
//! nothing here reaches the network or Claude. Ported from
//! test/tmux-sidebar.test.js.
//!
//! The environment is the process's, so every test holds `ENV` for its whole
//! run, and the file is a binary of its own, apart from tests/it, whose other
//! tests' threads a change to the environment would race.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use jiff::Zoned;
use jiff::tz::TimeZone;
use serde_json::{Value, json};
use switchboard::connector::ConnectorError;
use switchboard::day::calendar::{self, Feed, Look};
use switchboard::day::{
    Ask, AskOpts, Connector, Dirs, new_notes, slack_look, slack_thread, standup,
};
use switchboard::util;

static ENV: Mutex<()> = Mutex::new(());

/// A temporary HOME holding the state, notes, config and relay
/// directories. Holds `ENV` while it lives.
struct Home {
    _lock: MutexGuard<'static, ()>,
    root: PathBuf,
    dirs: Dirs,
}

impl Home {
    fn new(name: &str) -> Home {
        let lock = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!(
            "sb-day-{name}-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        let dirs = Dirs {
            state: root.join("state"),
            notes: root.join("Notes"),
            config: root.join("config"),
            relay: root.join("relay"),
        };
        for d in [&dirs.state, &dirs.notes, &dirs.config, &dirs.relay] {
            std::fs::create_dir_all(d).unwrap();
        }
        // SAFETY: every test holds ENV while it changes the environment.
        unsafe {
            std::env::set_var("HOME", &root);
            std::env::set_var("SWITCHBOARD_STATE_DIR", &dirs.state);
            std::env::set_var("SWITCHBOARD_SETTINGS", root.join("settings.json"));
        }
        Home {
            _lock: lock,
            root,
            dirs,
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn ny() -> TimeZone {
    TimeZone::get("America/New_York").unwrap()
}

fn at(y: i16, m: i8, d: i8, h: i8) -> Zoned {
    jiff::civil::date(y, m, d)
        .at(h, 0, 0, 0)
        .to_zoned(ny())
        .unwrap()
}

/// A connector answering as `answer` says, noting each call.
struct Fake<F> {
    asked: Mutex<Vec<(String, Value)>>,
    answer: F,
}

impl<F: Fn(&str, &Value) -> String + Sync> Fake<F> {
    fn new(answer: F) -> Fake<F> {
        Fake {
            asked: Mutex::new(Vec::new()),
            answer,
        }
    }

    fn tools(&self) -> Vec<String> {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .map(|(t, _)| t.rsplit("__").next().unwrap_or(t).to_string())
            .collect()
    }
}

impl<F: Fn(&str, &Value) -> String + Sync> Connector for Fake<F> {
    fn call(
        &self,
        tool: &str,
        input: Value,
        _timeout: Option<Duration>,
    ) -> impl Future<Output = Result<String, ConnectorError>> + Send {
        let out = (self.answer)(tool, &input);
        self.asked.lock().unwrap().push((tool.to_string(), input));
        async move { Ok(out) }
    }
}

/// Claude, answering as `answer` says, noting each prompt and model.
struct FakeAsk<F> {
    asked: Mutex<Vec<(String, &'static str)>>,
    answer: F,
}

impl<F: Fn() -> anyhow::Result<String> + Sync> FakeAsk<F> {
    fn new(answer: F) -> FakeAsk<F> {
        FakeAsk {
            asked: Mutex::new(Vec::new()),
            answer,
        }
    }
}

impl<F: Fn() -> anyhow::Result<String> + Sync> Ask for FakeAsk<F> {
    fn ask(
        &self,
        prompt: String,
        opts: AskOpts,
    ) -> impl Future<Output = anyhow::Result<String>> + Send {
        self.asked.lock().unwrap().push((prompt, opts.model));
        let out = (self.answer)();
        async move { out }
    }
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn the_standups_summary_is_asked_for_a_day_with_work_and_the_note_is_written_without_it_when_claude_cant()
 {
    let home = Home::new("standup");
    let dirs = &home.dirs;
    let repo = home.root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .env("GIT_AUTHOR_DATE", "2026-10-23T11:00:00-04:00")
            .env("GIT_COMMITTER_DATE", "2026-10-23T11:00:00-04:00")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "git {args:?}");
    };
    if util::which("git").is_none() {
        eprintln!("skipped: no git");
        return;
    }
    git(&["init", "-q"]);
    git(&["config", "user.email", "me@example.com"]);
    git(&["config", "user.name", "Me"]);
    git(&["remote", "add", "origin", "https://github.com/o/repo.git"]);
    git(&["commit", "-q", "--allow-empty", "-m", "Do a"]);
    let tabs = vec![repo.clone()];
    let parts = standup::Parts {
        github: false,
        linear: false,
        summary: true,
    };

    let claude = FakeAsk::new(|| Ok("I did a.".into()));
    util::block_on(standup::write_standup(
        dirs,
        &tabs,
        parts,
        &at(2026, 10, 26, 7),
        &claude,
    ))
    .unwrap();
    let asked = claude.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].1, "haiku");
    assert!(
        asked[0].0.contains("Commits in o/repo: Do a"),
        "{}",
        asked[0].0
    );
    assert!(
        read(dirs.notes.join("Standups/Standup 2026-10-26.md"))
            .contains("## Friday, Oct 23\n\nI did a.\n\n- Commits")
    );

    let down = FakeAsk::new(|| Err(anyhow::anyhow!("no Claude")));
    util::block_on(standup::write_standup(
        dirs,
        &tabs,
        parts,
        &at(2026, 10, 27, 7),
        &down,
    ))
    .unwrap();
    assert!(
        read(dirs.notes.join("Standups/Standup 2026-10-27.md"))
            .contains("## Monday, Oct 26\n\n- Nothing found"),
        "a day with nothing isn't summed up"
    );
    assert!(down.asked.lock().unwrap().is_empty());

    let off = standup::Parts {
        summary: false,
        ..parts
    };
    util::block_on(standup::write_standup(
        dirs,
        &tabs,
        off,
        &at(2026, 10, 28, 7),
        &down,
    ))
    .unwrap();
    assert!(
        down.asked.lock().unwrap().is_empty(),
        "not asked with the setting off"
    );

    std::fs::remove_file(dirs.notes.join("Standups/Standup 2026-10-26.md")).unwrap();
    util::block_on(standup::write_standup(
        dirs,
        &tabs,
        parts,
        &at(2026, 10, 26, 7),
        &down,
    ))
    .unwrap();
    assert!(
        read(dirs.notes.join("Standups/Standup 2026-10-26.md"))
            .contains("## Friday, Oct 23\n\n- Commits"),
        "written all the same without one"
    );
    let notes = new_notes::new_notes(&dirs.state, &dirs.notes);
    assert!(notes.iter().all(|n| n.source == "the standup") && notes.len() == 3);
}

#[test]
fn the_slack_look_searches_once_and_sorts_what_it_finds_as_slack_updates_does() {
    let home = Home::new("slack");
    let dirs = &home.dirs;
    std::fs::write(
        dirs.config.join("slack-updates.json"),
        r#"{"channels":["COPS"]}"#,
    )
    .unwrap();
    let now = util::now_ms();
    let secs = now / 1000;
    let result = |n: u32,
                  channel: &str,
                  place: &str,
                  from: &str,
                  user: &str,
                  ts: String,
                  thread: Option<&str>,
                  text: &str,
                  participants: &str| {
        format!(
            "### Result {n} of 6\nChannel: {place} (ID: {channel})\n{}From: {from} (ID: {user})\nTime: 2026-10-29 09:55:36 EDT\nMessage_ts: {ts}\nPermalink: [link](https://x.slack.com/archives/{channel}/p{}{})\nText: \n{text}\n\n---\n\n",
            if participants.is_empty() {
                String::new()
            } else {
                format!("Participants: {participants}\n")
            },
            ts.replace('.', ""),
            thread
                .map(|t| format!("?thread_ts={t}&cid={channel}"))
                .unwrap_or_default()
        )
    };
    let mine = format!("{}.000100", secs - 50);
    let page = [
        result(
            1,
            "DDEE",
            "DM",
            "Dee <dee@example.com>",
            "UDEE",
            format!("{}.000100", secs - 40),
            None,
            "did you see this?\nsecond line",
            "Me (ID: U0MEMEME1), Dee (ID: UDEE)",
        ),
        result(
            2,
            "CGEN",
            "#general",
            "Ada",
            "UADA",
            format!("{}.000100", secs - 30),
            None,
            "<@U0MEMEME1|Me> can you look",
            "",
        ),
        result(
            3,
            "CDEV",
            "#dev",
            "Me",
            "U0MEMEME1",
            mine.clone(),
            None,
            "my question",
            "",
        ),
        result(
            4,
            "CDEV",
            "#dev",
            "Lin",
            "ULIN",
            format!("{}.000100", secs - 20),
            Some(&mine),
            "an answer",
            "",
        ),
        result(
            5,
            "COPS",
            "#ops",
            "Bot",
            "UBOT2",
            format!("{}.000100", secs - 10),
            None,
            "deploy done",
            "",
        ),
        result(
            6,
            "CRANDOM",
            "#random",
            "Cy",
            "UCY",
            format!("{}.000100", secs - 5),
            None,
            "lunch?",
            "",
        ),
    ]
    .join("");
    let thread_mine = mine.clone();
    let slack = Fake::new(move |tool: &str, input: &Value| {
        if tool.ends_with("slack_read_user_profile") {
            "User ID: U0MEMEME1\nUsername: me".into()
        } else if tool.ends_with("slack_read_thread") {
            assert_eq!(input["channel_id"], "CDEV");
            assert_eq!(input["message_ts"], json!(thread_mine));
            json!({ "messages": format!("=== THREAD PARENT MESSAGE ===\nFrom: Me (U0MEMEME1)\nMessage TS: {thread_mine}\nmy question\nFiles: shot.png (ID: F1, image/png, 4 KB)\n\n=== THREAD REPLIES (1 total) ===\n\n--- Reply 1 of 1 ---\nFrom: Lin (ULIN)\nMessage TS: {}.000100\nan answer\n", secs - 20) }).to_string()
        } else {
            json!({ "results": format!("# Search Results for: \n\n## Messages (6 results)\n{page}") }).to_string()
        }
    });

    util::block_on(slack_look::look_slack(dirs, &slack, now)).unwrap();
    // The server reads the threads with something new after the look, in a
    // task of their own.
    util::block_on(slack_thread::read_wanted(
        &dirs.state,
        &slack,
        &Default::default(),
        now,
    ))
    .unwrap();
    assert_eq!(
        slack.tools(),
        [
            "slack_read_user_profile",
            "slack_search_public_and_private",
            "slack_read_thread"
        ],
        "one profile, one search for everything, then the thread with something new"
    );
    let threads: Value =
        serde_json::from_str(&read(dirs.state.join("slack-threads.json"))).unwrap();
    let thread = &threads[format!("CDEV:{mine}")];
    assert_eq!(thread["replies"], 1);
    assert_eq!(thread["messages"][0]["files"][0]["id"], "F1");
    assert_eq!(thread["messages"][1]["text"], "an answer");
    let saved: Value = serde_json::from_str(&read(dirs.state.join("slack.json"))).unwrap();
    let kind = |text: &str| {
        saved["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["text"] == text)
            .map(|i| i["kind"].as_str().unwrap().to_string())
    };
    assert_eq!(
        kind("did you see this?\nsecond line").as_deref(),
        Some("dm"),
        "its lines kept, for drawing as Slack does"
    );
    assert_eq!(kind("@Me can you look").as_deref(), Some("mention"));
    assert_eq!(
        kind("an answer").as_deref(),
        Some("thread"),
        "a reply in a thread you started"
    );
    assert_eq!(kind("deploy done").as_deref(), Some("channel"));
    assert_eq!(kind("lunch?"), None, "a channel not chosen is left out");
    assert_eq!(kind("my question"), None, "what you wrote isn't shown");
    let dm = saved["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "dm")
        .unwrap();
    assert_eq!(dm["where"], "Dee");
    assert_eq!(dm["from"], "Dee", "names without their email");
    assert_eq!(
        saved["replied"][format!("CDEV:{mine}")],
        json!(mine),
        "but counts as having read that thread"
    );

    util::block_on(slack_look::look_slack(dirs, &slack, now + 60_000)).unwrap();
    util::block_on(slack_thread::read_wanted(
        &dirs.state,
        &slack,
        &Default::default(),
        now + 60_000,
    ))
    .unwrap();
    assert_eq!(
        slack
            .tools()
            .iter()
            .filter(|t| *t == "slack_read_user_profile")
            .count(),
        1,
        "your member ID is kept"
    );
    assert_eq!(
        slack
            .tools()
            .iter()
            .filter(|t| *t == "slack_read_thread")
            .count(),
        1,
        "a thread with nothing newer isn't read again"
    );
}

#[test]
fn the_calendar_view_shows_the_day_today_saved_and_says_when_it_is_out_of_date() {
    let home = Home::new("calendar");
    let dirs = &home.dirs;
    let now = at(2026, 11, 2, 12);
    let rfc = |h: i8| format!("2026-11-02T{h:02}:00:00-05:00");
    let day = json!({ "date": "2026-11-02", "events": [
        { "title": "Planning", "start": rfc(9), "end": rfc(10), "you": "going", "link": "https://calendar.example/1" },
        { "title": "Review", "start": rfc(13), "end": rfc(14), "you": "maybe", "join": "https://zoom.example/j/2" },
        { "title": "Bad link", "start": rfc(12), "end": rfc(12), "you": "going", "join": "javascript:alert(1)" },
        { "title": "Offsite", "allDay": true, "you": "nonsense" },
        { "title": 7 },
    ] });
    assert_eq!(
        calendar::save_today(&dirs.state, &day.to_string()).unwrap(),
        "Saved 4 events for mission control."
    );
    let saved: Value = serde_json::from_str(&read(dirs.state.join("calendar.json"))).unwrap();
    assert_eq!(
        saved["events"][2]["join"],
        Value::Null,
        "only https links are kept"
    );
    assert_eq!(saved["events"][3]["you"], "not answered");
    let mut cache = calendar::Saved::default();
    let feed = Feed::default();
    let view = calendar::calendar_view(&Look {
        feed: &feed,
        saved: cache.read(&dirs.state),
        looping: false,
        now: &now,
        moves: &Default::default(),
    });
    let titles: Vec<&str> = view.items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Offsite", "Planning", "Bad link", "Review"],
        "all day first, then by start"
    );
    assert_eq!(view.items[1].detail, "9 – 10 AM");
    assert_eq!(
        view.items[3].target,
        switchboard::items::Target::Url("https://zoom.example/j/2".into()),
        "Enter joins the call"
    );
    assert_eq!(view.items[0].group.as_deref(), Some("All day"));
    assert_eq!(view.error, None);

    let mut old = saved.clone();
    old["date"] = json!("2020-01-01");
    std::fs::write(dirs.state.join("calendar.json"), old.to_string()).unwrap();
    // A different modified time, so the cache reads it again.
    let later = std::time::SystemTime::now() + Duration::from_secs(5);
    std::fs::File::options()
        .write(true)
        .open(dirs.state.join("calendar.json"))
        .unwrap()
        .set_modified(later)
        .unwrap();
    let stale = calendar::calendar_view(&Look {
        feed: &feed,
        saved: cache.read(&dirs.state),
        looping: false,
        now: &now,
        moves: &Default::default(),
    });
    assert!(stale.items.is_empty());
    assert_eq!(
        stale.error.as_deref(),
        Some(
            "No calendar for today yet (last saved 2020-01-01): resume the Calendar routine in the Routines view to fetch it."
        )
    );
    assert!(calendar::calendar_stale(
        &feed,
        cache.read(&dirs.state),
        "2026-11-02",
        now.timestamp().as_millisecond()
    ));
    std::fs::remove_file(dirs.state.join("calendar.json")).unwrap();
    let none = calendar::calendar_view(&Look {
        feed: &feed,
        saved: cache.read(&dirs.state),
        looping: false,
        now: &now,
        moves: &Default::default(),
    });
    assert!(none.setup.is_some(), "nothing set up says how to set it up");
}
