//! The relay's behaviour end to end, ported from test/slack-relay.test.js:
//! messages handled, tabs typed into and followed, alerts, the Home tab,
//! screens, buttons and forms, notices, the watchdog. Each runs a relay
//! with stand-ins for Slack, the tabs and Haiku (src/relay/testing.rs), so
//! none reaches Slack, a server, or your ~/.claude.

use std::time::Duration;

use serde_json::{Value, json};

use super::commands::{handle, named};
use super::config::{PICKUP_WARN_MS, REPLY_WAIT_MS};
use super::dashboard::{DOING_EVERY_MS, Refresh, refresh_dashboard};
use super::followups::{follow_ups, new_events};
use super::slack::{Message, press_from, submission_from};
use super::state::{Follow, Home, Target};
use super::testing::Rig;
use super::{alerts, run, tabs};
use crate::util;

fn message(ts: &str, text: &str, thread: Option<&str>) -> Message {
    Message {
        ts: ts.into(),
        text: text.into(),
        thread_ts: thread.map(str::to_string),
        ..Message::default()
    }
}

fn now_ts(n: u32) -> String {
    format!("{}.{n:06}", util::now_secs())
}

fn ticks(rig: &Rig) {
    let tabs = tabs::claude_tabs(&rig.cx);
    let mut events = new_events(&rig.cx);
    rig.block_on(follow_ups(&rig.cx, &tabs, &mut events));
}

fn alert(rig: &Rig, tab: &str, index: i64, name: &str) -> String {
    let ts = rig.ts_ago(60, 100);
    rig.cx.state.borrow_mut().alerts.insert(
        ts.clone(),
        Target {
            window: tab.into(),
            index,
            name: name.into(),
            thread: None,
        },
    );
    ts
}

fn follow(thread: Option<&str>, index: i64, text: &str) -> Follow {
    Follow {
        thread: thread.map(str::to_string),
        index,
        name: "work".into(),
        text: text.into(),
        sent_at: util::now_ms() - 5_000,
        ..Follow::default()
    }
}

#[test]
fn a_top_level_message_sent_with_the_picker_goes_on_in_the_tabs_home_thread() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    let ts = now_ts(1);
    rig.block_on(handle(&rig.cx, message(&ts, "hello from slack", None)))
        .unwrap();
    assert!(
        !rig.screen(&tab).contains("hello from slack"),
        "not typed anywhere yet"
    );
    let posts = rig.posts();
    assert!(posts[0].body.contains("Send it to a tab?"));
    let button = posts[0]
        .buttons
        .iter()
        .find(|b| {
            serde_json::from_str::<Value>(b["value"].as_str().unwrap()).unwrap()["w"]
                == tab.as_str()
        })
        .unwrap();
    let value: Value = serde_json::from_str(button["value"].as_str().unwrap()).unwrap();
    let press = super::slack::Press {
        value,
        message: Some(super::slack::Pressed {
            ts: posts[0].ts.clone(),
            thread_ts: None,
            text: posts[0].body.clone(),
        }),
        ..Default::default()
    };
    rig.block_on(super::presses::handle_press(&rig.cx, press))
        .unwrap();
    assert!(rig.screen(&tab).contains("hello from slack"));
    // The tab's home thread starts; nothing else is posted until the
    // answer, which goes there and to the main DM.
    let posts = rig.posts();
    assert_eq!(posts.len(), 2);
    let home = posts[1].ts.clone();
    assert!(posts[1].body.starts_with(&format!(
        "🤖 **Tab {index}, work**\n⚪ Idle\n\nThis is its thread"
    )));
    assert_eq!(posts[1].thread, None);
    let state = rig.cx.state.borrow();
    assert_eq!(state.homes[&tab].ts, home);
    assert_eq!(state.alerts[&home].window, tab);
    let f = &state.following[&tab];
    assert!(f.broadcast);
    assert_eq!(f.text, "hello from slack");
    assert_eq!(f.thread.as_deref(), Some(home.as_str()));
    assert_eq!(f.request.as_deref(), Some(ts.as_str()));
}

#[test]
fn a_reply_under_an_alert_goes_to_the_alerts_tab_as_one_paste() {
    let rig = Rig::new();
    let (tab, pane) = rig.claude_tab("work");
    let thread = alert(&rig, &tab, rig.index_of(&tab), "work");
    rig.block_on(handle(
        &rig.cx,
        message(&now_ts(2), "line one\nline two", Some(&thread)),
    ))
    .unwrap();
    assert!(rig.screen(&tab).ends_with("line one\nline two\n"));
    let sent = rig.mux.sent.borrow().clone();
    assert_eq!(
        sent,
        [
            format!("{pane} paste line one\nline two"),
            format!("{pane} key Enter")
        ],
        "one paste, then Enter"
    );
    assert!(
        rig.posts().is_empty(),
        "👀 says it was taken; nothing is posted"
    );
    assert_eq!(
        rig.cx.state.borrow().following[&tab].thread.as_deref(),
        Some(thread.as_str())
    );
}

#[test]
fn a_message_to_a_prompt_in_vim_s_normal_mode_presses_i_first() {
    let rig = Rig::new();
    let (tab, pane) = rig.claude_tab("work");
    let thread = alert(&rig, &tab, rig.index_of(&tab), "work");
    rig.set_screen(&tab, "╭──╮\n│ >│\n╰──╯\n  -- NORMAL --\n");
    rig.block_on(handle(&rig.cx, message(&now_ts(2), "go on", Some(&thread))))
        .unwrap();
    assert_eq!(
        rig.mux.sent.borrow().clone(),
        [
            format!("{pane} type i"),
            format!("{pane} paste go on"),
            format!("{pane} key Enter")
        ],
        "Insert mode first, so the message arrives whole"
    );
}

const DRAFTED: &str =
    "⏺ Done.\n────────\n❯ send that to #stream\n────────\n  Opus 5.5 | Context: 10.0% used\n";

/// A reply sent to a tab whose prompt holds a draft, giving the question
/// the relay posted and a press of one of its buttons.
fn asked_about_draft(rig: &Rig, tab: &str) -> (super::testing::Post, Vec<Value>) {
    let thread = alert(rig, tab, rig.index_of(tab), "work");
    rig.set_screen(tab, DRAFTED);
    rig.block_on(handle(&rig.cx, message(&now_ts(2), "go on", Some(&thread))))
        .unwrap();
    let asked = rig.posts().last().unwrap().clone();
    let values = asked
        .buttons
        .iter()
        .map(|b| serde_json::from_str(b["value"].as_str().unwrap()).unwrap())
        .collect();
    (asked, values)
}

fn press_on(rig: &Rig, asked: &super::testing::Post, value: Value) {
    let press = super::slack::Press {
        value,
        message: Some(super::slack::Pressed {
            ts: asked.ts.clone(),
            thread_ts: asked.thread.clone(),
            text: asked.body.clone(),
        }),
        ..Default::default()
    };
    rig.block_on(super::presses::handle_press(&rig.cx, press))
        .unwrap();
}

#[test]
fn a_message_to_a_tab_with_an_unsent_draft_asks_first_and_types_nothing() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let (asked, _) = asked_about_draft(&rig, &tab);
    assert!(rig.mux.sent.borrow().is_empty(), "nothing typed");
    assert!(
        asked.body.contains("> send that to #stream"),
        "{}",
        asked.body
    );
    let names: Vec<&str> = asked
        .buttons
        .iter()
        .map(|b| b["text"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Replace it", "Send after it", "Keep it"]);
    let thread = asked.thread.clone().unwrap();
    assert_eq!(
        rig.cx.state.borrow().alerts[&thread].window,
        tab,
        "the thread leads to the tab while you decide"
    );
}

#[test]
fn send_after_it_types_the_message_after_the_draft() {
    let rig = Rig::new();
    let (tab, pane) = rig.claude_tab("work");
    let (asked, values) = asked_about_draft(&rig, &tab);
    press_on(&rig, &asked, values[1].clone());
    assert_eq!(
        rig.mux.sent.borrow().clone(),
        [format!("{pane} paste go on"), format!("{pane} key Enter")]
    );
    assert!(
        rig.cx.state.borrow().following.contains_key(&tab),
        "followed as any message typed in"
    );
}

#[test]
fn replace_it_clears_the_draft_then_types_the_message() {
    let rig = Rig::new();
    let (tab, pane) = rig.claude_tab("work");
    let (asked, values) = asked_about_draft(&rig, &tab);
    press_on(&rig, &asked, values[0].clone());
    assert_eq!(
        rig.mux.sent.borrow().clone(),
        [
            format!("{pane} key C-u"),
            format!("{pane} key C-u"),
            format!("{pane} paste go on"),
            format!("{pane} key Enter")
        ]
    );
    assert!(!rig.screen(&tab).contains("send that"));
}

#[test]
fn keep_it_types_nothing_and_says_so() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let (asked, values) = asked_about_draft(&rig, &tab);
    press_on(&rig, &asked, values[2].clone());
    assert!(rig.mux.sent.borrow().is_empty());
    assert!(
        rig.posts()
            .last()
            .unwrap()
            .body
            .contains("Kept the draft in tab")
    );
    // Pressed again, the message is no longer held.
    press_on(&rig, &asked, values[1].clone());
    assert!(rig.mux.sent.borrow().is_empty());
    assert!(
        rig.posts()
            .last()
            .unwrap()
            .body
            .contains("I no longer have that message")
    );
}

#[test]
fn a_press_once_the_draft_is_gone_types_the_message_and_says_so() {
    let rig = Rig::new();
    let (tab, pane) = rig.claude_tab("work");
    let (asked, values) = asked_about_draft(&rig, &tab);
    rig.set_screen(&tab, &DRAFTED.replace("❯ send that to #stream", "❯"));
    press_on(&rig, &asked, values[0].clone());
    assert_eq!(
        rig.mux.sent.borrow().clone(),
        [format!("{pane} paste go on"), format!("{pane} key Enter")],
        "no Ctrl-U with nothing to clear"
    );
    assert!(rig.posts().iter().any(|p| p.body.contains("was gone")));
}

#[test]
fn a_newer_message_typed_at_once_drops_the_one_waiting_on_a_draft() {
    let rig = Rig::new();
    let (tab, pane) = rig.claude_tab("work");
    let (asked, values) = asked_about_draft(&rig, &tab);
    rig.set_screen(&tab, &DRAFTED.replace("❯ send that to #stream", "❯"));
    let thread = asked.thread.clone().unwrap_or(asked.ts.clone());
    rig.block_on(handle(&rig.cx, message(&now_ts(3), "newer", Some(&thread))))
        .unwrap();
    press_on(&rig, &asked, values[1].clone());
    assert_eq!(
        rig.mux.sent.borrow().clone(),
        [format!("{pane} paste newer"), format!("{pane} key Enter")],
        "the older message isn't typed after the newer one"
    );
}

#[test]
fn a_held_message_is_never_taken_as_the_answer_to_a_later_choice() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let (asked, values) = asked_about_draft(&rig, &tab);
    rig.set_screen(&tab, "Allow rm -rf build?\n❯ 1. Yes\n  2. No\n");
    rig.set_state(&tab, "waiting", "1791500200");
    press_on(&rig, &asked, values[1].clone());
    assert!(rig.mux.sent.borrow().is_empty(), "nothing pressed");
    assert!(
        rig.posts()
            .last()
            .unwrap()
            .body
            .contains("is showing a choice now")
    );
}

#[test]
fn a_numbered_list_in_an_idle_tabs_answer_doesnt_stop_a_held_message() {
    let rig = Rig::new();
    let (tab, pane) = rig.claude_tab("work");
    let (asked, values) = asked_about_draft(&rig, &tab);
    rig.set_screen(&tab, &format!("❯ 1. First\n  2. Second\n{DRAFTED}"));
    press_on(&rig, &asked, values[1].clone());
    assert_eq!(
        rig.mux.sent.borrow().clone(),
        [format!("{pane} paste go on"), format!("{pane} key Enter")]
    );
}

#[test]
fn a_dimmed_suggestion_in_the_prompt_isnt_a_draft() {
    let rig = Rig::new();
    let (tab, pane) = rig.claude_tab("work");
    let thread = alert(&rig, &tab, rig.index_of(&tab), "work");
    rig.set_screen(
        &tab,
        &DRAFTED.replace("❯ send that to #stream", "❯ ⟨Try \"fix lint\"⟩"),
    );
    rig.block_on(handle(&rig.cx, message(&now_ts(2), "go on", Some(&thread))))
        .unwrap();
    assert_eq!(
        rig.mux.sent.borrow().clone(),
        [format!("{pane} paste go on"), format!("{pane} key Enter")]
    );
}

#[test]
fn with_the_draft_check_off_the_message_is_typed_after_the_draft() {
    let mut rig = Rig::new();
    std::rc::Rc::get_mut(&mut rig.cx).unwrap().settings =
        Box::new(|k| (k == "relay_draft_check").then_some(json!(false)));
    let (tab, pane) = rig.claude_tab("work");
    let thread = alert(&rig, &tab, rig.index_of(&tab), "work");
    rig.set_screen(&tab, DRAFTED);
    rig.block_on(handle(&rig.cx, message(&now_ts(2), "go on", Some(&thread))))
        .unwrap();
    assert_eq!(
        rig.mux.sent.borrow().clone(),
        [format!("{pane} paste go on"), format!("{pane} key Enter")]
    );
}

#[test]
fn unrecognised_messages_ones_naming_a_tab_and_old_ones_are_not_typed_anywhere() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    let now = util::now_secs();
    rig.block_on(async {
        handle(&rig.cx, message(&format!("{now}.000003"), "l: yes", None))
            .await
            .unwrap();
        handle(
            &rig.cx,
            message(&format!("{now}.000004"), &format!("{index}: yes"), None),
        )
        .await
        .unwrap();
        handle(
            &rig.cx,
            message(&format!("{}.000005", now - 600), "yes", None),
        )
        .await
        .unwrap();
        handle(
            &rig.cx,
            message(
                &format!("{now}.000006"),
                "just chatting",
                Some("1791500000.000200"),
            ),
        )
        .await
        .unwrap();
    });
    let posts = rig.posts();
    assert!(
        posts[0]
            .body
            .contains("That doesn't name a tab or a command. **Send it to a tab?**")
    );
    assert!(posts[0].buttons.iter().any(|b| {
        b["text"]["text"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{index} "))
    }));
    assert!(
        posts[1]
            .body
            .contains("That doesn't name a tab or a command"),
        "a number in front names no tab"
    );
    assert!(
        posts[2]
            .body
            .contains("I missed this while I wasn't running")
    );
    assert!(posts[3].body.contains("This thread isn't linked to a tab"));
    assert_eq!(posts[3].thread.as_deref(), Some("1791500000.000200"));
    assert!(!rig.screen(&tab).contains("yes") && !rig.screen(&tab).contains("chatting"));
    assert!(rig.cx.state.borrow().following.is_empty());
}

#[test]
fn status_lists_the_claude_tabs_with_their_states() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_state(&tab, "waiting", "");
    rig.block_on(handle(&rig.cx, message(&now_ts(7), "status", None)))
        .unwrap();
    let body = &rig.posts()[0].body;
    assert!(body.contains(&format!(
        "| **{}** | work | 🔴 **waiting on you** |",
        rig.index_of(&tab)
    )));
}

#[test]
fn new_starts_a_tab_running_claude_with_the_prompt() {
    let rig = Rig::new();
    let ts = rig
        .block_on(handle(
            &rig.cx,
            message(&now_ts(8), "new: fix the 'login' bug", None),
        ))
        .unwrap()
        .unwrap();
    let windows = rig.mux.windows.borrow();
    let (w, panes) = windows.last().unwrap();
    assert_eq!(panes[0].screen, "claude 'fix the '\\''login'\\'' bug'\n");
    assert_eq!(
        w.recorded.as_deref(),
        Some(panes[0].info.id.as_str()),
        "the pane Claude starts in is the tab's"
    );
    let state = rig.cx.state.borrow();
    assert_eq!(state.alerts[&ts].window, w.id);
    assert_eq!(
        state.homes[&w.id].ts, ts,
        "started from the top level, its home"
    );
}

#[test]
fn new_in_a_place_that_isnt_one_starts_nothing() {
    let rig = Rig::new();
    rig.block_on(handle(
        &rig.cx,
        message(&now_ts(9), "new ~/no-such-dir: nothing", None),
    ))
    .unwrap();
    rig.block_on(handle(
        &rig.cx,
        message(&now_ts(10), "new zzz: nothing", None),
    ))
    .unwrap();
    let posts = rig.posts();
    assert!(
        posts[0]
            .body
            .contains("There's no directory `~/no-such-dir`")
    );
    assert!(posts[1].body.contains("Nothing matches `zzz`"));
    assert!(rig.mux.opened.borrow().is_empty());
}

#[test]
fn new_in_a_worktree_of_someone_elses_pr_starts_nothing() {
    let rig = Rig::new();
    // As wtpr --untrusted checks a PR out, in the review folder.
    std::fs::create_dir_all(rig.home().join(".cache/switchboard/pr-review/o-r/12345")).unwrap();
    rig.block_on(handle(
        &rig.cx,
        message(
            &now_ts(12),
            "new ~/.cache/switchboard/pr-review/o-r/12345: look around",
            None,
        ),
    ))
    .unwrap();
    assert!(
        rig.posts()[0]
            .body
            .contains("holds someone else's PR, checked out untrusted")
    );
    assert!(rig.mux.opened.borrow().is_empty());
}

#[test]
fn new_in_a_typed_path_starts_there_as_typed_links_and_all() {
    let rig = Rig::new();
    let real = rig.root.join("elsewhere");
    std::fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, rig.home().join("linked")).unwrap();
    rig.block_on(handle(
        &rig.cx,
        message(&now_ts(11), "new ~/linked/./: look around", None),
    ))
    .unwrap();
    assert_eq!(
        rig.mux.opened.borrow()[0].0,
        rig.home().join("linked"),
        "the link kept, its `.` worked out"
    );
    assert!(rig.posts()[0].body.contains("`~/linked`"));
}

#[test]
fn events_carry_on_from_the_saved_offset_and_leave_half_written_lines() {
    let rig = Rig::new();
    std::fs::write(
        rig.cx.paths.events_file(),
        "{\"event\":\"done\",\"time\":1}\n{\"event\":\"waiting\",\"time\":2}\n{\"event\":\"do",
    )
    .unwrap();
    let events: Vec<String> = new_events(&rig.cx).into_iter().map(|e| e.event).collect();
    assert_eq!(events, ["done", "waiting"]);
    assert!(new_events(&rig.cx).is_empty());
    let offset = rig.cx.state.borrow().events_offset;
    rig.append_events(&[]);
    let mut file = util::read_text(rig.cx.paths.events_file()).unwrap();
    file.push_str("ne\",\"time\":3}\n");
    std::fs::write(rig.cx.paths.events_file(), file).unwrap();
    rig.cx.state.borrow_mut().events_offset = offset;
    assert_eq!(
        new_events(&rig.cx)
            .iter()
            .map(|e| e.time)
            .collect::<Vec<_>>(),
        [3]
    );
    // A file shorter than the offset is a new one, read from the start.
    rig.events(&[json!({ "event": "done", "time": 4 })]);
    assert_eq!(
        new_events(&rig.cx)
            .iter()
            .map(|e| e.time)
            .collect::<Vec<_>>(),
        [4]
    );
}

#[test]
fn a_finished_turn_is_reported_once_in_the_requests_thread() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    let transcript = rig.transcript(&tab, "followed", &[
        json!({ "type": "user", "message": { "content": "deploy it" } }),
        json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Deployed." }] } }),
    ]);
    let now = util::now_secs();
    rig.events(&[
        // Before the request was sent, so not its answer.
        json!({ "event": "done", "time": now - 120, "transcript_path": transcript }),
        // Another session's turn.
        json!({ "event": "done", "time": now, "transcript_path": "/elsewhere/other.jsonl" }),
        json!({ "event": "done", "time": now, "transcript_path": transcript }),
    ]);
    let thread = "1791500000.000300";
    rig.cx
        .state
        .borrow_mut()
        .following
        .insert(tab.clone(), follow(Some(thread), index, "deploy it"));
    ticks(&rig);
    ticks(&rig);
    let posts = rig.posts();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].thread.as_deref(), Some(thread));
    assert_eq!(
        posts[0].body,
        format!("🤖 tab {index}, work is done:\n> deploy it\n\nDeployed.")
    );
    assert!(rig.cx.state.borrow().following.is_empty());
    assert_eq!(rig.cx.state.borrow().alerts[&posts[0].ts].window, tab);
}

#[test]
fn a_wait_shows_the_screen_so_a_permission_prompt_can_be_answered() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_screen(&tab, "Do you want to proceed? 1. Yes 2. No\n");
    let transcript = rig.transcript(
        &tab,
        "waiting",
        &[json!({ "type": "user", "message": { "content": "delete the branch" } })],
    );
    rig.events(&[
        json!({ "event": "waiting", "time": util::now_secs(), "transcript_path": transcript }),
    ]);
    rig.cx.state.borrow_mut().following.insert(
        tab.clone(),
        follow(None, rig.index_of(&tab), "delete the branch"),
    );
    ticks(&rig);
    let posts = rig.posts();
    assert_eq!(posts.len(), 1);
    assert!(posts[0].body.contains("is waiting on you"));
    assert!(
        posts[0]
            .body
            .ends_with("On screen now:\n```\nDo you want to proceed? 1. Yes 2. No\n```")
    );
}

#[test]
fn alerts_come_once_per_state_change_with_the_screen_for_a_wait() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    rig.set_state(&tab, "waiting", "100");
    rig.set_screen(&tab, "Allow this edit? 1. Yes 2. No\n");
    rig.transcript(
        &tab,
        "alerted",
        &[json!({ "type": "user", "message": { "content": "edit the config" } })],
    );
    let run = |rig: &Rig| {
        let tabs = tabs::claude_tabs(&rig.cx);
        rig.block_on(alerts::alert_tabs(&rig.cx, &tabs, &[]));
    };
    run(&rig);
    run(&rig);
    rig.set_state(&tab, "done", "200");
    run(&rig);
    // The home thread, then each alert in it with a one-line notice also in
    // the main DM.
    let posts = rig.posts();
    assert_eq!(posts.len(), 4);
    let home = &posts[0].ts;
    assert!(posts[0].body.contains("This is its thread"));
    assert!(rig.cx.state.borrow().threads.contains_key(home));
    assert!(
        posts[1]
            .body
            .starts_with(&format!("🤖 **Tab {index}, work** is **waiting on you**"))
    );
    assert!(posts[1].body.contains("> edit the config"));
    assert!(
        posts[1]
            .body
            .contains("On screen now:\n```\nAllow this edit? 1. Yes 2. No")
    );
    assert_eq!(posts[1].thread.as_ref(), Some(home));
    assert!(!posts[1].broadcast);
    assert_eq!(
        posts[2].body,
        format!("🤖 **Tab {index}, work** is **waiting on you**.")
    );
    assert!(posts[2].broadcast);
    assert!(posts[3].body.contains("has finished"));
    assert!(!posts[3].body.contains("On screen now:"));
    assert!(
        !posts[3].broadcast,
        "a finished turn gets no line in the main DM"
    );
}

#[test]
fn a_turn_another_tabs_message_started_is_posted_with_who_sent_it() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let (sender, _) = rig.claude_tab("design");
    let sender_index = rig.index_of(&sender);
    let address = "uds:/tmp/scenario-design.sock";
    {
        let mut windows = rig.mux.windows.borrow_mut();
        let (_, panes) = windows.iter_mut().find(|(w, _)| w.id == sender).unwrap();
        panes[0].info.address = address.into();
    }
    rig.transcript(
        &tab,
        "messaged",
        &[
            json!({ "type": "user", "message": { "content": "walk me through it" } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "The walkthrough." }] } }),
            json!({ "type": "user", "isMeta": true, "origin": { "kind": "peer", "from": address, "body": "Here is my proposal" }, "message": { "content": "Another Claude session sent a message:\n<cross-session-message>Here is my proposal</cross-session-message>" } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "The proposal looks right." }] } }),
        ],
    );
    rig.set_state(&tab, "done", "200");
    let run = |rig: &Rig| {
        let tabs = tabs::claude_tabs(&rig.cx);
        rig.block_on(alerts::alert_tabs(&rig.cx, &tabs, &[]));
    };
    run(&rig);
    run(&rig);
    let finished: Vec<_> = rig
        .posts()
        .into_iter()
        .filter(|p| p.body.contains("has finished"))
        .collect();
    assert_eq!(finished.len(), 1);
    let body = &finished[0].body;
    assert!(
        body.contains(&format!(
            "Tab {sender_index}, design sent: Here is my proposal"
        )),
        "{body}"
    );
    assert!(body.contains("The proposal looks right."), "{body}");
    assert!(!body.contains("The walkthrough."), "{body}");
}

#[test]
fn a_loop_tab_is_alerted_when_it_waits_not_each_time_it_finishes_a_tick() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_loop(&tab);
    rig.set_state(&tab, "done", "100");
    let run = |rig: &Rig| {
        let tabs = tabs::claude_tabs(&rig.cx);
        rig.block_on(alerts::alert_tabs(&rig.cx, &tabs, &[]));
    };
    run(&rig);
    assert!(rig.posts().is_empty());
    rig.set_state(&tab, "waiting", "200");
    run(&rig);
    assert!(
        rig.posts()
            .iter()
            .any(|p| p.body.contains("is **waiting on you**"))
    );
}

#[test]
fn a_message_queued_behind_a_busy_tabs_turn_gets_an_hourglass_and_nothing_is_posted() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_state(&tab, "working", "");
    let thread = alert(&rig, &tab, rig.index_of(&tab), "work");
    let ts = now_ts(9);
    rig.block_on(handle(
        &rig.cx,
        message(&ts, "and also this", Some(&thread)),
    ))
    .unwrap();
    assert_eq!(
        rig.reactions(),
        [(ts.clone(), "hourglass_flowing_sand".to_string())]
    );
    assert!(rig.posts().is_empty());
    let state = rig.cx.state.borrow();
    assert_eq!(state.following[&tab].request.as_deref(), Some(ts.as_str()));
    assert!(!state.following[&tab].taken);
}

#[test]
fn once_the_tab_takes_your_message_eyes_and_a_working_note_until_the_answer() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_state(&tab, "working", "");
    let transcript = rig.transcript(
        &tab,
        "taken",
        &[json!({ "type": "user", "message": { "content": "rename   the\nmodule" } })],
    );
    let now = util::now_secs();
    rig.events(&[
        json!({ "event": "prompt", "time": now, "transcript_path": transcript, "prompt": "something else" }),
        json!({ "event": "prompt", "time": now, "transcript_path": transcript, "prompt": "rename   the\nmodule" }),
    ]);
    let request = format!("{}.000100", now - 5);
    rig.cx.state.borrow_mut().following.insert(
        tab.clone(),
        Follow {
            request: Some(request.clone()),
            ..follow(Some(&request), rig.index_of(&tab), "rename the module")
        },
    );
    ticks(&rig);
    ticks(&rig);
    assert_eq!(rig.reactions(), [(request.clone(), "eyes".to_string())]);
    assert!(rig.cx.state.borrow().following[&tab].taken);
    let posts = rig.posts();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].body, "🤖 _Working on it._");
    assert_eq!(posts[0].thread.as_deref(), Some(request.as_str()));
    let buttons: Vec<(String, String)> = posts[0]
        .buttons
        .iter()
        .map(|b| {
            let v: Value = serde_json::from_str(b["value"].as_str().unwrap()).unwrap();
            (
                b["text"]["text"].as_str().unwrap().to_string(),
                v["c"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        buttons,
        [
            ("Progress".into(), "progress".into()),
            ("Stop".into(), "esc".into())
        ]
    );
    // The answer: the note goes, and the turn is posted with ✅.
    let mut text = util::read_text(&transcript).unwrap();
    text.push_str(&format!(
        "\n{}",
        json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Renamed." }] } })
    ));
    std::fs::write(&transcript, text).unwrap();
    rig.append_events(&[
        json!({ "event": "done", "time": now + 1, "transcript_path": transcript }),
    ]);
    ticks(&rig);
    assert_eq!(rig.calls("chat.delete")[0]["ts"], posts[0].ts.as_str());
    assert!(rig.posts().last().unwrap().body.contains("is done:"));
    assert!(rig.posts().last().unwrap().body.contains("Renamed."));
    assert!(
        rig.reactions()
            .contains(&(request, "white_check_mark".to_string()))
    );
}

#[test]
fn an_idle_tab_that_hasnt_taken_the_message_is_reported_once_with_its_screen() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_screen(&tab, "Trust this folder? 1. Yes 2. No\n");
    let request = rig.ts_ago(40, 100);
    rig.cx.state.borrow_mut().following.insert(
        tab.clone(),
        Follow {
            request: Some(request.clone()),
            sent_at: util::now_ms() - PICKUP_WARN_MS - 1_000,
            ..follow(Some(&request), rig.index_of(&tab), "start")
        },
    );
    ticks(&rig);
    ticks(&rig);
    let posts = rig.posts();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].thread.as_deref(), Some(request.as_str()));
    assert!(posts[0].body.contains(&format!(
        "tab {}, work hasn't taken your message after 30 seconds",
        rig.index_of(&tab)
    )));
    assert!(posts[0].body.contains("Trust this folder? 1. Yes 2. No"));
    assert!(rig.reactions().is_empty());
}

#[test]
fn esc_presses_escape_only_in_a_tab_in_the_middle_of_something() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    let thread = alert(&rig, &tab, index, "work");
    rig.cx
        .state
        .borrow_mut()
        .following
        .insert(tab.clone(), follow(None, index, "long job"));
    rig.block_on(handle(&rig.cx, message(&now_ts(11), "esc", Some(&thread))))
        .unwrap();
    rig.set_state(&tab, "working", "");
    rig.block_on(handle(&rig.cx, message(&now_ts(12), "ESC", Some(&thread))))
        .unwrap();
    let posts = rig.posts();
    assert!(
        posts[0]
            .body
            .contains("isn't in the middle of anything, so I didn't press Escape")
    );
    assert_eq!(posts[0].thread.as_deref(), Some(thread.as_str()));
    assert!(
        posts[1]
            .body
            .starts_with(&format!("🤖 Pressed Escape in tab {index}, work"))
    );
    assert_eq!(rig.screen(&tab).matches("^[").count(), 1);
    assert!(rig.cx.state.borrow().following.is_empty());
    assert_eq!(rig.cx.state.borrow().alerts[&posts[1].ts].window, tab);
}

#[test]
fn a_finished_turn_whose_reply_isnt_written_yet_waits_for_it() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    let transcript = rig.transcript(
        &tab,
        "late",
        &[json!({ "type": "user", "message": { "content": "give me the list" } })],
    );
    rig.events(&[
        json!({ "event": "done", "time": util::now_secs(), "transcript_path": transcript }),
    ]);
    let thread = rig.ts_ago(5, 100);
    rig.cx.state.borrow_mut().following.insert(
        tab.clone(),
        follow(Some(&thread), index, "give me the list"),
    );
    // The Stop hook has fired, but the reply hasn't been written yet.
    ticks(&rig);
    assert!(rig.posts().is_empty());
    let mut text = util::read_text(&transcript).unwrap();
    text.push_str(&format!(
        "\n{}",
        json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "1. Verify" }] } })
    ));
    std::fs::write(&transcript, text).unwrap();
    ticks(&rig);
    assert_eq!(
        rig.posts()[0].body,
        format!("🤖 tab {index}, work is done:\n> give me the list\n\n1. Verify")
    );
    // A reply that never comes is posted as it is once the wait is over.
    std::fs::write(
        &transcript,
        json!({ "type": "user", "message": { "content": "and the rest" } }).to_string(),
    )
    .unwrap();
    rig.append_events(&[
        json!({ "event": "done", "time": util::now_secs(), "transcript_path": transcript }),
    ]);
    rig.cx
        .state
        .borrow_mut()
        .following
        .insert(tab.clone(), follow(Some(&thread), index, "and the rest"));
    ticks(&rig);
    assert_eq!(rig.posts().len(), 1);
    if let Some(e) = rig
        .cx
        .state
        .borrow_mut()
        .following
        .get_mut(&tab)
        .and_then(|f| f.ended.as_mut())
    {
        e.at -= REPLY_WAIT_MS + 1_000;
    }
    ticks(&rig);
    assert_eq!(rig.posts().len(), 2);
    assert!(rig.posts()[1].body.ends_with("(no reply yet)"));
}

#[test]
fn a_lagging_transcript_is_posted_with_the_hooks_final_message() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let transcript = rig.transcript(
        &tab,
        "lagging",
        &[json!({ "type": "user", "message": { "content": "sum it up" } })],
    );
    rig.events(&[json!({ "event": "done", "time": util::now_secs(), "transcript_path": transcript, "last": "Here is the sum." })]);
    rig.cx
        .state
        .borrow_mut()
        .following
        .insert(tab.clone(), follow(None, rig.index_of(&tab), "sum it up"));
    ticks(&rig);
    assert!(
        rig.posts().is_empty(),
        "waited for while the transcript catches up"
    );
    if let Some(e) = rig
        .cx
        .state
        .borrow_mut()
        .following
        .get_mut(&tab)
        .and_then(|f| f.ended.as_mut())
    {
        e.at -= REPLY_WAIT_MS + 1_000;
    }
    ticks(&rig);
    assert!(
        rig.posts()[0]
            .body
            .ends_with("> sum it up\n\nHere is the sum.")
    );
}

#[test]
fn progress_summarizes_what_a_tab_is_doing_without_typing_into_it() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    rig.set_state(&tab, "working", "");
    let started = jiff::Timestamp::from_millisecond(util::now_ms() - 4 * 60_000)
        .unwrap()
        .to_string();
    let lately = jiff::Timestamp::now().to_string();
    rig.transcript(&tab, "busy", &[
        json!({ "type": "user", "timestamp": started, "message": { "content": "add the progress command" } }),
        json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Reading the relay first." }, { "type": "tool_use", "id": "t1", "name": "Read", "input": { "file_path": "bin/slack-relay" } }] } }),
        json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "..." }] } }),
        json!({ "type": "assistant", "message": { "content": [{ "type": "tool_use", "id": "t2", "name": "Bash", "input": { "command": "node --test", "description": "Run the tests" } }] } }),
        json!({ "type": "user", "timestamp": lately, "message": { "content": [{ "type": "tool_result", "tool_use_id": "t2", "is_error": true, "content": "1 failing" }] } }),
    ]);
    *rig.haiku.answer.borrow_mut() = Some("• Running the tests, one is failing.".into());
    let thread = alert(&rig, &tab, index, "work");
    let request = now_ts(13);
    rig.block_on(handle(
        &rig.cx,
        message(&request, "progress", Some(&thread)),
    ))
    .unwrap();
    assert_eq!(rig.reactions(), [(request, "eyes".to_string())]);
    let posts = rig.posts();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].thread.as_deref(), Some(thread.as_str()));
    assert_eq!(
        posts[0].body,
        format!(
            "🤖 tab {index}, work is working, on a turn started 4m ago, last activity just now, 2 tools so far.\n• Running the tests, one is failing."
        )
    );
    let given = rig.haiku.given.borrow()[0].0.clone();
    assert!(given.contains("The prompt: add the progress command"));
    assert!(given.contains("Read: bin/slack-relay"));
    assert!(given.contains("Bash: Run the tests"));
    assert!(given.contains("(that failed: 1 failing)"));
    assert!(!rig.screen(&tab).contains("progress"));
    assert!(rig.cx.state.borrow().following.is_empty());
    // What it cost, counted for the Home tab.
    assert_eq!(super::summaries::haiku_today(&rig.cx.paths).calls, 1);
}

#[test]
fn progress_falls_back_to_the_last_few_steps_when_the_summary_fails() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.transcript(&tab, "busy2", &[
        json!({ "type": "user", "message": { "content": "deploy" } }),
        json!({ "type": "assistant", "timestamp": jiff::Timestamp::now().to_string(), "message": { "content": [{ "type": "tool_use", "id": "t1", "name": "Bash", "input": { "command": "make deploy" } }] } }),
    ]);
    let thread = alert(&rig, &tab, rig.index_of(&tab), "work");
    rig.block_on(handle(
        &rig.cx,
        message(&now_ts(14), "progress", Some(&thread)),
    ))
    .unwrap();
    assert!(rig.posts()[0].body.ends_with(
        "is idle, last activity just now, 1 tool so far.\nI couldn't summarize it, so here is what it did last:\n• Bash: make deploy"
    ));
}

#[test]
fn progress_all_lists_every_tabs_facts_when_the_summary_fails() {
    let rig = Rig::new();
    let (first, _) = rig.claude_tab("one");
    let (second, _) = rig.claude_tab("two");
    rig.set_state(&first, "waiting", "");
    rig.set_state(&second, "working", "");
    rig.block_on(handle(&rig.cx, message(&now_ts(18), "progress all", None)))
        .unwrap();
    let body = &rig.posts()[0].body;
    assert!(body.starts_with(
        "🤖 2 Claude tabs:\nI couldn't summarize them, so here is where each one is:\n"
    ));
    assert!(body.contains(&format!(
        "• tab {}, one is **waiting on you**",
        rig.index_of(&first)
    )));
    assert!(body.contains(&format!("• tab {}, two is working", rig.index_of(&second))));
}

#[test]
fn the_home_tab_is_published_then_again_only_when_it_changes_or_you_open_it() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    rig.set_state(&tab, "done", &(util::now_secs() - 120).to_string());
    {
        let mut state = rig.cx.state.borrow_mut();
        state.dm.as_mut().unwrap().url = Some("https://example.slack.com/".into());
        state
            .threads
            .insert("1791500000.000300".into(), "1791500000.000300".into());
        state.homes.insert(
            tab.clone(),
            Home {
                ts: "1791500000.000300".into(),
                pid: rig.pid_of(&tab),
                ..Home::default()
            },
        );
        // The dashboard message the relay used to pin in the DM.
        state.dashboard = Some(json!({ "ts": "1791500000.000900" }));
    }
    let quick = Refresh {
        quick: true,
        ..Refresh::default()
    };
    rig.block_on(refresh_dashboard(&rig.cx, quick)).unwrap();
    rig.block_on(refresh_dashboard(&rig.cx, quick)).unwrap();
    assert_eq!(rig.calls("chat.delete").len(), 1, "the old message, once");
    let views = rig.calls("views.publish");
    assert_eq!(views.len(), 1, "nothing changed the second time");
    assert_eq!(views[0]["user_id"], "U1");
    let blocks = views[0]["view"]["blocks"].as_array().unwrap().clone();
    let at = blocks
        .iter()
        .position(|b| {
            b["text"]["text"]
                .as_str()
                .is_some_and(|t| t.starts_with(&format!("*{index}*")))
        })
        .unwrap();
    assert!(
        blocks[at]["text"]["text"]
            .as_str()
            .unwrap()
            .contains("✅ finished for 2m")
    );
    assert_eq!(
        blocks[at + 1]["elements"].as_array().unwrap().len(),
        1,
        "only Open"
    );
    // The tab's screen has its thread and Close.
    let t = tabs::tab(&rig.cx, &tab).unwrap();
    let screen = super::screens::tab_screen(&rig.cx, &t, None);
    let actions = screen["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["type"] == "actions")
        .unwrap();
    assert_eq!(actions["elements"][2]["text"]["text"], "Go to thread");
    assert_eq!(
        actions["elements"][2]["url"],
        "https://example.slack.com/archives/D1/p1791500000000300?thread_ts=1791500000.000300&cid=D1"
    );
    rig.set_state(&tab, "waiting", &(util::now_secs() - 120).to_string());
    rig.block_on(refresh_dashboard(&rig.cx, quick)).unwrap();
    assert_eq!(rig.calls("views.publish").len(), 2);
    rig.block_on(refresh_dashboard(&rig.cx, Refresh::opened_quick()))
        .unwrap();
    assert_eq!(
        rig.calls("views.publish").len(),
        3,
        "opening the Home tab shows it as it is now"
    );
}

#[test]
fn the_home_tabs_lines_are_written_again_only_when_a_tab_changes() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_state(&tab, "done", &(util::now_secs() - 60).to_string());
    rig.transcript(&tab, "summed", &[json!({ "type": "user", "timestamp": jiff::Timestamp::now().to_string(), "message": { "content": "write the explainer" } })]);
    *rig.haiku.answer.borrow_mut() = Some("Published the explainer; nothing left to do.".into());
    let beat = Refresh {
        heartbeat: true,
        ..Refresh::default()
    };
    rig.block_on(refresh_dashboard(&rig.cx, beat)).unwrap();
    let asked = || rig.haiku.given.borrow().len();
    assert_eq!(asked(), 1);
    let view = rig.calls("views.publish").last().unwrap().clone();
    let last = view["view"]["blocks"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["elements"][0]["text"], "Haiku today: 1 call, $0.00");
    rig.block_on(refresh_dashboard(&rig.cx, beat)).unwrap();
    assert_eq!(asked(), 1, "nothing changed");
    rig.set_state(&tab, "working", &(util::now_secs() - 60).to_string());
    rig.block_on(refresh_dashboard(&rig.cx, beat)).unwrap();
    assert_eq!(asked(), 1, "not within a minute");
    rig.cx.state.borrow_mut().doing.get_mut(&tab).unwrap().at -= DOING_EVERY_MS;
    rig.block_on(refresh_dashboard(&rig.cx, beat)).unwrap();
    assert_eq!(asked(), 2);
    // A closed tab's line goes.
    rig.mux.windows.borrow_mut().clear();
    rig.block_on(refresh_dashboard(&rig.cx, beat)).unwrap();
    assert!(rig.cx.state.borrow().doing.is_empty());
}

#[test]
fn the_home_tab_puts_what_needs_you_first() {
    let rig = Rig::new();
    let row = |index: i64, state: &str, since: i64| super::dashboard::Row {
        tab: tabs::Tab {
            id: format!("@{index}"),
            index,
            name: format!("tab{index}"),
            state: state.into(),
            since: since.to_string(),
            pid: "1".into(),
            ..Default::default()
        },
        lasted: String::new(),
        last: String::new(),
        context: String::new(),
        doing: String::new(),
        choices: Vec::new(),
    };
    let rows = vec![
        row(1, "idle", 100),
        row(2, "working", 200),
        row(3, "done", 300),
        row(4, "waiting", 400),
        row(5, "done", 250),
        row(6, "waiting", 350),
        row(7, "working", 150),
    ];
    let view = super::dashboard::dashboard_view(&rig.cx, &rows, 0, None);
    let order: Vec<String> = view["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| match b["type"].as_str() {
            Some("header") => b["text"]["text"].as_str().map(str::to_string),
            Some("section") => b["text"]["text"]
                .as_str()
                .filter(|t| t.starts_with('*') && t[1..].starts_with(|c: char| c.is_ascii_digit()))
                .map(|t| t[1..t[1..].find('*').unwrap() + 1].to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(
        order,
        [
            "Claude tabs",
            "Needs you (4)",
            "6",
            "4",
            "5",
            "3",
            "Working (2)",
            "2",
            "7",
            "Idle (1)",
            "1"
        ]
    );
    // A stopped relay marks it out of date, with nothing to refresh.
    let stopped = super::dashboard::dashboard_view(&rig.cx, &rows, 0, Some(util::now_ms()));
    let checkin = stopped["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["block_id"] == "checkin")
        .unwrap();
    assert!(
        checkin["text"]["text"]
            .as_str()
            .unwrap()
            .starts_with("⚪ The relay stopped at ")
    );
    assert!(checkin.get("accessory").is_none());
}

/// The Home's headers and tab numbers, top to bottom.
fn home_order(view: &Value) -> Vec<String> {
    view["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| match b["type"].as_str() {
            Some("header") => b["text"]["text"].as_str().map(str::to_string),
            Some("section") => b["text"]["text"]
                .as_str()
                .filter(|t| t.starts_with('*') && t[1..].starts_with(|c: char| c.is_ascii_digit()))
                .map(|t| t[1..t[1..].find('*').unwrap() + 1].to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_home_tab_groups_as_the_sidebar_does() {
    let rig = Rig::new();
    let row = |index: i64, stage: (&str, &str, u64, u64)| super::dashboard::Row {
        tab: tabs::Tab {
            id: format!("@{index}"),
            index,
            name: format!("tab{index}"),
            state: "idle".into(),
            pid: "1".into(),
            stage: Some(super::mux::TabStage {
                key: stage.0.into(),
                title: stage.1.into(),
                rank: stage.2,
                place: stage.3,
            }),
            ..Default::default()
        },
        lasted: String::new(),
        last: String::new(),
        context: String::new(),
        doing: String::new(),
        choices: Vec::new(),
    };
    let rows = vec![
        row(1, ("later", "Reminders", 13, 1)),
        row(2, ("idle", "Idle", 12, 0)),
        row(3, ("review", "Review loop", 4, 0)),
        row(4, ("later", "Reminders", 13, 0)),
        row(5, ("merged", "Merged", 11, 0)),
        row(6, ("idle", "Idle", 12, 1)),
    ];
    let view = super::dashboard::dashboard_view(&rig.cx, &rows, 0, None);
    assert_eq!(
        home_order(&view),
        [
            "Claude tabs",
            "Review loop (1)",
            "3",
            "Merged (1)",
            "5",
            "Idle (2)",
            "2",
            "6",
            "Reminders (2)",
            "4",
            "1"
        ]
    );
}

#[test]
fn a_published_home_leaves_out_the_tabs_waiting_on_a_reminder() {
    let rig = Rig::new();
    let (working, _) = rig.claude_tab("working");
    rig.set_stage(&working, "working", "Working", 5, 0);
    let mut later = Vec::new();
    for name in ["later1", "later2"] {
        let (tab, _) = rig.claude_tab(name);
        rig.set_stage(&tab, "later", "Reminders", 13, later.len() as u64);
        rig.set_state(&tab, "done", &(util::now_secs() - 60).to_string());
        rig.transcript(&tab, name, &[json!({ "type": "user", "timestamp": jiff::Timestamp::now().to_string(), "message": { "content": "write it" } })]);
        later.push(tab);
    }
    *rig.haiku.answer.borrow_mut() = Some("Done.".into());
    let beat = Refresh {
        heartbeat: true,
        ..Refresh::default()
    };
    rig.block_on(refresh_dashboard(&rig.cx, beat)).unwrap();
    assert_eq!(
        rig.haiku.given.borrow().len(),
        0,
        "no line for a reminder tab"
    );
    assert!(rig.cx.state.borrow().doing.is_empty());
    let view = rig.calls("views.publish").last().unwrap()["view"].clone();
    assert_eq!(
        home_order(&view),
        [
            "Claude tabs".to_string(),
            "Working (1)".into(),
            rig.index_of(&working).to_string()
        ]
    );
    let checkin = view["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["block_id"] == "checkin")
        .unwrap();
    assert!(
        checkin["text"]["text"]
            .as_str()
            .unwrap()
            .ends_with(" · 2 tabs waiting on a reminder"),
        "{checkin}"
    );
}

#[test]
fn a_published_home_lists_a_reminder_tab_under_reminders_after_idle() {
    let mut rig = Rig::new();
    std::rc::Rc::get_mut(&mut rig.cx).unwrap().settings =
        Box::new(|k| (k == "relay_home_reminders").then_some(json!(true)));
    let (idle, _) = rig.claude_tab("idle");
    let (later, _) = rig.claude_tab("later");
    rig.set_stage(&idle, "idle", "Idle", 12, 0);
    rig.set_stage(&later, "later", "Reminders", 13, 0);
    rig.set_remind_at(&later, util::now_secs() + 2 * 3600);
    let quick = Refresh {
        quick: true,
        ..Refresh::default()
    };
    rig.block_on(refresh_dashboard(&rig.cx, quick)).unwrap();
    let view = rig.calls("views.publish").last().unwrap()["view"].clone();
    let (i, l) = (rig.index_of(&idle), rig.index_of(&later));
    assert_eq!(
        home_order(&view),
        [
            "Claude tabs".to_string(),
            "Idle (1)".into(),
            i.to_string(),
            "Reminders (1)".into(),
            l.to_string()
        ]
    );
    let shown = view["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| b["text"]["text"].as_str())
        .find(|t| t.starts_with(&format!("*{l}*")))
        .unwrap();
    assert!(shown.contains(" · remind in 2h"), "{shown}");
}

#[test]
fn an_answer_to_a_waiting_tabs_choice_presses_just_the_number() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    rig.set_screen(
        &tab,
        "Do you want to proceed?\n❯ 1. Yes\n  2. Yes, and don't ask again\n  3. No (esc)\n",
    );
    rig.set_state(&tab, "waiting", "100");
    let thread = alert(&rig, &tab, index, "work");
    rig.block_on(handle(
        &rig.cx,
        message(&now_ts(16), "yes, and", Some(&thread)),
    ))
    .unwrap();
    assert!(
        rig.screen(&tab).ends_with("(esc)\n2"),
        "no Enter after the number"
    );
    assert!(rig.posts()[0].body.starts_with(&format!(
        "🤖 Chose 2. Yes, and don't ask again in tab {index}, work."
    )));
    assert!(rig.cx.state.borrow().following[&tab].taken);
}

#[test]
fn a_reply_that_isnt_one_of_a_waiting_prompts_options_types_nothing() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_screen(
        &tab,
        "Allow rm -rf build?\n❯ 1. Yes\n  2. No, and tell Claude what to do differently\n",
    );
    rig.set_state(&tab, "waiting", "100");
    let thread = alert(&rig, &tab, rig.index_of(&tab), "work");
    let before = rig.screen(&tab);
    rig.block_on(handle(
        &rig.cx,
        message(&now_ts(20), "cancel", Some(&thread)),
    ))
    .unwrap();
    let posts = rig.posts();
    assert!(posts[0].body.contains(
        "is showing a choice, and that isn't one of its options, so I didn't type anything"
    ));
    assert!(posts[0].body.contains("1. Yes\n2. No, and tell Claude"));
    assert_eq!(
        posts[0].buttons.len(),
        2,
        "the choice's buttons, a tap away"
    );
    assert_eq!(rig.screen(&tab), before, "nothing typed, no Enter");
    assert!(rig.cx.state.borrow().following.is_empty());
}

#[test]
fn a_reply_in_a_thread_names_its_tab_by_its_number_now_not_when_the_thread_began() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_screen(
        &tab,
        "Allow rm -rf build?\n❯ 1. Yes\n  2. No, and tell Claude what to do differently\n",
    );
    rig.set_state(&tab, "waiting", "100");
    let now = rig.index_of(&tab);
    // Alerted while a tab ahead of it was still open, and under an old name.
    let thread = alert(&rig, &tab, now + 1, "old name");
    rig.block_on(handle(
        &rig.cx,
        message(&now_ts(20), "cancel", Some(&thread)),
    ))
    .unwrap();
    let posts = rig.posts();
    assert!(
        posts[0]
            .body
            .contains(&format!("{} is showing a choice", tabs::label(now, "work"))),
        "{}",
        posts[0].body
    );

    // A tab whose Claude has exited is still named as it is now.
    let (other, _) = rig.claude_tab("shell");
    rig.mux
        .windows
        .borrow_mut()
        .iter_mut()
        .find(|(w, _)| w.id == other)
        .unwrap()
        .1[0]
        .info
        .claude = false;
    let exited = named(&rig.cx, &other, 9, "old name");
    assert_eq!(
        (exited.index, exited.name.as_str()),
        (rig.index_of(&other), "shell")
    );

    // A tab that is gone is named as the alert stored it.
    let gone = named(&rig.cx, "@999", 7, "closed");
    assert_eq!((gone.index, gone.name.as_str()), (7, "closed"));
}

#[test]
fn a_dialog_before_a_session_starts_is_alerted_once_and_answered_with_arrow_keys() {
    let rig = Rig::new();
    let (tab, pane) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    rig.set_screen(&tab, " Is this a project you trust?\n ❯ No, exit\n   Yes, I trust this folder\n Enter to confirm · Esc to cancel\n");
    let run = |rig: &Rig| {
        let tabs = tabs::claude_tabs(&rig.cx);
        rig.block_on(alerts::alert_tabs(&rig.cx, &tabs, &[]));
    };
    run(&rig);
    rig.cx.mem.borrow_mut().alerts.dialog_checked.clear();
    run(&rig);
    let posts = rig.posts();
    assert_eq!(
        posts.len(),
        3,
        "the thread, the alert in it, and a notice in the main DM, once"
    );
    assert!(posts[1].body.starts_with(&format!(
        "🤖 **Tab {index}, work** is showing a dialog before its session starts"
    )));
    assert!(posts[1].body.contains(
        "**Is this a project you trust?**\n1. No, exit (selected)\n2. Yes, I trust this folder\n"
    ));
    assert!(posts[2].broadcast);
    rig.block_on(handle(
        &rig.cx,
        message(&now_ts(21), "yes", posts[1].thread.as_deref()),
    ))
    .unwrap();
    assert!(
        rig.posts()
            .last()
            .unwrap()
            .body
            .starts_with("🤖 Chose \"Yes, I trust this folder\" in tab")
    );
    let sent = rig.mux.sent.borrow().clone();
    assert_eq!(
        sent,
        [format!("{pane} key Down"), format!("{pane} key Enter")]
    );
}

#[test]
fn a_tab_with_no_state_is_checked_for_a_dialog_every_few_seconds_not_every_second() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let run = |rig: &Rig| {
        let tabs = tabs::claude_tabs(&rig.cx);
        rig.block_on(alerts::alert_tabs(&rig.cx, &tabs, &[]));
    };
    run(&rig);
    rig.set_screen(&tab, " Is this a project you trust?\n ❯ No, exit\n   Yes, I trust this folder\n Enter to confirm · Esc to cancel\n");
    run(&rig);
    assert!(
        rig.posts().is_empty(),
        "a second later, the screen isn't read again"
    );
    rig.cx
        .mem
        .borrow_mut()
        .alerts
        .dialog_checked
        .insert(tab.clone(), util::now_ms() - alerts::DIALOG_CHECK_MS);
    run(&rig);
    assert!(rig.posts().iter().any(|p| {
        p.body
            .contains("is showing a dialog before its session starts")
    }));
}

#[test]
fn a_waiting_tabs_alert_has_its_options_as_buttons_and_a_press_answers_only_that_prompt() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_screen(&tab, "Allow rm -rf build?\n❯ 1. Yes\n  2. No\n");
    rig.set_state(&tab, "waiting", "1791500200");
    let tabs_now = tabs::claude_tabs(&rig.cx);
    rig.block_on(alerts::alert_tabs(&rig.cx, &tabs_now, &[]));
    let posts = rig.posts();
    let sent = posts
        .iter()
        .find(|p| p.body.contains("is **waiting on you**. Reply here"))
        .unwrap()
        .clone();
    let names: Vec<&str> = sent
        .buttons
        .iter()
        .map(|b| b["text"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["1. Yes", "2. No"]);
    assert_eq!(sent.buttons[0]["style"], "primary");
    let yes: Value = serde_json::from_str(sent.buttons[0]["value"].as_str().unwrap()).unwrap();
    let pressed = super::slack::Pressed {
        ts: sent.ts.clone(),
        thread_ts: sent.thread.clone(),
        text: sent.body.clone(),
    };
    let press = |value: Value| super::slack::Press {
        value,
        message: Some(pressed.clone()),
        ..Default::default()
    };
    // Made for an earlier wait, or another prompt: refused, nothing pressed.
    let mut earlier = yes.clone();
    earlier["s"] = json!("1791500100");
    rig.block_on(super::presses::handle_press(&rig.cx, press(earlier)))
        .unwrap();
    assert!(
        rig.posts()
            .last()
            .unwrap()
            .body
            .contains("That choice isn't showing any more")
    );
    let mut other = yes.clone();
    other["k"] = json!("0000000000000000");
    rig.block_on(super::presses::handle_press(&rig.cx, press(other)))
        .unwrap();
    assert!(
        rig.posts()
            .last()
            .unwrap()
            .body
            .contains("That choice isn't showing any more")
    );
    assert!(!rig.screen(&tab).ends_with('1'));
    rig.block_on(super::presses::handle_press(&rig.cx, press(yes)))
        .unwrap();
    assert!(rig.screen(&tab).ends_with("\n1"));
    let edited = rig
        .calls("chat.update")
        .into_iter()
        .find(|u| u["ts"] == sent.ts.as_str())
        .unwrap();
    assert!(
        edited["text"]
            .as_str()
            .unwrap()
            .ends_with("_Chose **1. Yes**._")
    );
    assert_eq!(
        edited["blocks"].as_array().unwrap().len(),
        1,
        "no buttons left to press twice"
    );
}

#[test]
fn a_press_on_a_prompt_with_a_countdown_answers_it_while_the_countdown_runs() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let asking = |left: &str| {
        format!(
            "Dangerous rm operation\n⚠️ Claude Code will automatically block this request in {left}, to avoid holding up an unattended session\n❯ 1. Yes\n  2. No\n"
        )
    };
    rig.set_screen(&tab, &asking("1:03"));
    rig.set_state(&tab, "waiting", "1791500200");
    let tabs_now = tabs::claude_tabs(&rig.cx);
    rig.block_on(alerts::alert_tabs(&rig.cx, &tabs_now, &[]));
    let sent = rig
        .posts()
        .iter()
        .find(|p| p.body.contains("is **waiting on you**. Reply here"))
        .unwrap()
        .clone();
    let yes: Value = serde_json::from_str(sent.buttons[0]["value"].as_str().unwrap()).unwrap();
    rig.set_screen(&tab, &asking("0:51"));
    rig.block_on(super::presses::handle_press(
        &rig.cx,
        super::slack::Press {
            value: yes,
            message: Some(super::slack::Pressed {
                ts: sent.ts.clone(),
                thread_ts: sent.thread.clone(),
                text: sent.body.clone(),
            }),
            ..Default::default()
        },
    ))
    .unwrap();
    assert!(rig.screen(&tab).ends_with("\n1"));
}

#[test]
fn a_button_made_for_one_tab_answers_that_tab_even_in_a_thread_linked_to_another() {
    let rig = Rig::new();
    let (mine, _) = rig.claude_tab("mine");
    let (other, _) = rig.claude_tab("other");
    rig.set_screen(&mine, "Allow rm -rf build?\n❯ 1. Yes\n  2. No\n");
    rig.set_state(&mine, "waiting", "1791500400");
    let thread = alert(&rig, &other, rig.index_of(&other), "other");
    rig.cx
        .state
        .borrow_mut()
        .threads
        .insert(thread.clone(), thread.clone());
    let tab = tabs::tab(&rig.cx, &mine).unwrap();
    let found = tabs::options_in(&tabs::screen(&rig.cx, &mine)).unwrap();
    let yes = super::buttons::option_buttons(&tab, &found)[0]
        .value
        .clone()
        .unwrap();
    let press = super::slack::Press {
        value: yes,
        message: Some(super::slack::Pressed {
            ts: now_ts(401),
            thread_ts: Some(thread),
            text: "🤖 Tab, waiting".into(),
        }),
        ..Default::default()
    };
    rig.block_on(super::presses::handle_press(&rig.cx, press))
        .unwrap();
    assert!(
        rig.screen(&mine).ends_with("\n1"),
        "the tab it was made for got the answer"
    );
    assert!(!rig.screen(&other).contains('1'));
    assert!(
        rig.posts()
            .last()
            .unwrap()
            .body
            .contains(&format!("in tab {}, ", rig.index_of(&mine)))
    );
}

#[test]
fn a_finished_turn_that_asks_gets_quick_replies_and_a_stale_one_isnt_typed() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_state(&tab, "done", "300");
    rig.transcript(&tab, "asking", &[
        json!({ "type": "user", "message": { "content": "merge it" } }),
        json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Squash merge or rebase?" }] } }),
    ]);
    *rig.haiku.answer.borrow_mut() =
        Some(r#"{"kind":"choice","options":["Squash merge","Rebase"]}"#.into());
    let tabs_now = tabs::claude_tabs(&rig.cx);
    rig.block_on(alerts::alert_tabs(&rig.cx, &tabs_now, &[]));
    let turn = rig
        .posts()
        .into_iter()
        .find(|p| p.body.contains("has finished"))
        .unwrap();
    let names: Vec<&str> = turn
        .buttons
        .iter()
        .map(|b| b["text"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["Skill"],
        "posted at once, with nothing that answers for you"
    );
    let edit = rig
        .calls("chat.update")
        .into_iter()
        .find(|u| u["ts"] == turn.ts.as_str())
        .unwrap();
    let quick: Vec<Value> = edit["blocks"][1]["elements"].as_array().unwrap().clone();
    let names: Vec<&str> = quick
        .iter()
        .map(|b| b["text"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Squash merge", "Rebase", "Skill"]);
    // Claude says more: the reply is out of date.
    rig.transcript(&tab, "asking", &[
        json!({ "type": "user", "message": { "content": "merge it" } }),
        json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Never mind, merged." }] } }),
    ]);
    let value: Value = serde_json::from_str(quick[0]["value"].as_str().unwrap()).unwrap();
    let press = super::slack::Press {
        value,
        message: Some(super::slack::Pressed {
            ts: turn.ts.clone(),
            thread_ts: turn.thread.clone(),
            text: turn.body.clone(),
        }),
        ..Default::default()
    };
    rig.block_on(super::presses::handle_press(&rig.cx, press))
        .unwrap();
    assert!(
        rig.posts()
            .last()
            .unwrap()
            .body
            .contains("that reply is out of date")
    );
    assert!(!rig.screen(&tab).contains("Squash"));
}

#[test]
fn milestones_ping_a_tabs_thread_on_a_commit_and_a_push() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    let transcript = rig.transcript(
        &tab,
        "shipping",
        &[json!({ "type": "user", "message": { "content": "an old turn" } })],
    );
    let run = |rig: &Rig| {
        let tabs = tabs::claude_tabs(&rig.cx);
        rig.block_on(super::milestones::milestones(&rig.cx, &tabs));
    };
    // First sight: followed from here, the old turn is not news.
    run(&rig);
    assert!(rig.posts().is_empty());
    let lines: String = [
        json!({ "type": "assistant", "message": { "content": [{ "type": "tool_use", "id": "c1", "name": "Bash", "input": { "command": "git commit -m 'Add two'" } }] } }),
        json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "c1", "content": "[feature abc1234] Add two\n 1 file changed" }] } }),
        json!({ "type": "assistant", "message": { "content": [{ "type": "tool_use", "id": "t1", "name": "Bash", "input": { "command": "pnpm test" } }] } }),
        json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t1", "is_error": true, "content": "FAIL app.test.js" }] } }),
        json!({ "type": "assistant", "message": { "content": [{ "type": "tool_use", "id": "p1", "name": "Bash", "input": { "command": "git push -u origin feature" } }] } }),
        json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "p1", "content": "To github.com:o/r.git\n * [new branch]      feature -> feature" }] } }),
    ]
    .iter()
    .map(|e| format!("\n{e}"))
    .collect();
    let before = util::read_text(&transcript).unwrap();
    std::fs::write(&transcript, before + &lines + "\n").unwrap();
    run(&rig);
    let posts = rig.posts();
    let bodies: Vec<&str> = posts[1..].iter().map(|p| p.body.as_str()).collect();
    assert_eq!(
        bodies,
        [
            format!("🤖 tab {index}, work: committed `abc1234` Add two on `feature`"),
            format!("🤖 tab {index}, work: pushed `feature`"),
        ],
        "a failing test run isn't news"
    );
    let home = rig.cx.state.borrow().homes[&tab].ts.clone();
    assert!(
        posts[1..]
            .iter()
            .all(|p| p.thread.as_deref() == Some(home.as_str()) && !p.broadcast)
    );
}

#[test]
fn while_youre_at_the_computer_threads_get_everything_and_only_the_main_dms_lines_wait() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_screen(&tab, "Allow rm -rf build?\n❯ 1. Yes\n  2. No\n");
    rig.set_state(&tab, "waiting", &util::now_secs().to_string());
    rig.at_computer();
    let t = tabs::tab(&rig.cx, &tab).unwrap();
    let tabs_now = tabs::claude_tabs(&rig.cx);
    rig.block_on(alerts::alert_tabs(&rig.cx, &tabs_now, &[]));
    rig.block_on(super::milestones::ping(
        &rig.cx,
        &t,
        "a push failed: rejected",
        super::sending::Kind::Failures,
        true,
    ))
    .unwrap();
    rig.block_on(super::milestones::ping(
        &rig.cx,
        &t,
        "pushed `feature`",
        super::sending::Kind::Milestones,
        false,
    ))
    .unwrap();
    assert!(
        rig.posts().iter().all(|p| !p.broadcast),
        "nothing in the main DM yet"
    );
    assert_eq!(
        rig.cx.presence.held.borrow().len(),
        2,
        "the wait's line and the failure's"
    );
    rig.set_reading(Some(super::presence::AWAY_AFTER_MS), Some(false));
    rig.block_on(super::presence::flush(&rig.cx));
    let lines: Vec<String> = rig
        .posts()
        .into_iter()
        .filter(|p| p.broadcast)
        .map(|p| p.body)
        .collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].ends_with("is **waiting on you**."));
    assert!(lines[1].contains("a push failed"));
    // Away: a failure is one message, in the thread and the main DM at once.
    rig.block_on(super::milestones::ping(
        &rig.cx,
        &t,
        "a push failed again",
        super::sending::Kind::Failures,
        true,
    ))
    .unwrap();
    assert!(rig.posts().last().unwrap().broadcast);
}

#[test]
fn a_working_tab_with_nothing_new_for_ten_minutes_is_noted_once_per_stall() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    rig.set_state(&tab, "working", "");
    rig.set_screen(&tab, "✻ Running pnpm test… (11m 2s)\n");
    let stale = jiff::Timestamp::from_millisecond(util::now_ms() - super::stuck::STUCK_MS - 60_000)
        .unwrap()
        .to_string();
    rig.transcript(&tab, "stalled", &[
        json!({ "type": "user", "timestamp": stale, "message": { "content": "run the suite" } }),
        json!({ "type": "assistant", "timestamp": stale, "message": { "content": [{ "type": "tool_use", "id": "b1", "name": "Bash", "input": { "command": "pnpm test" } }] } }),
        // Bookkeeping Claude Code writes to an idle session: not activity.
        json!({ "type": "pr-link", "timestamp": jiff::Timestamp::now().to_string() }),
    ]);
    let run = |rig: &Rig| {
        let tabs = tabs::claude_tabs(&rig.cx);
        rig.block_on(super::stuck::stuck_tabs(&rig.cx, &tabs));
    };
    run(&rig);
    run(&rig);
    let posts = rig.posts();
    assert_eq!(
        posts.len(),
        3,
        "the thread, the note, and a line in the main DM, once"
    );
    assert!(posts[1].body.starts_with(&format!(
        "🤖 tab {index}, work may be stuck: it's marked working, but nothing has happened for 11m."
    )));
    assert!(
        posts[1]
            .body
            .contains("What it did last:\n• Bash: pnpm test")
    );
    assert!(
        posts[1]
            .body
            .contains("On screen now:\n```\n✻ Running pnpm test")
    );
    assert!(posts[2].broadcast);
    assert!(
        posts[2]
            .body
            .ends_with("may be stuck: nothing new for 11m.")
    );
    // Not working any more: forgotten.
    rig.set_state(&tab, "done", "");
    run(&rig);
    assert_eq!(rig.posts().len(), 3);
    assert!(rig.cx.state.borrow().stuck.is_empty());
    // Just marked working after a long idle: no stall.
    rig.set_state(&tab, "working", &util::now_secs().to_string());
    run(&rig);
    assert_eq!(rig.posts().len(), 3);
    // Just after the computer wakes, nothing is stuck.
    rig.set_state(&tab, "working", "");
    rig.cx.life.borrow_mut().woke_at = util::now_ms();
    run(&rig);
    assert_eq!(rig.posts().len(), 3);
}

#[test]
fn a_turn_ending_in_the_tab_in_front_of_you_still_goes_in_its_thread_once() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_focused(&tab, true);
    let transcript = rig.transcript(&tab, "front", &[
        json!({ "type": "user", "message": { "content": "look" } }),
        json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Looked." }] } }),
    ]);
    let now = util::now_secs();
    rig.events(&[
        json!({ "event": "done", "time": now, "transcript_path": transcript, "last": "Looked." }),
    ]);
    rig.block_on(run::tab_tick(&rig.cx));
    let finished: Vec<_> = rig
        .posts()
        .into_iter()
        .filter(|p| p.body.contains("has finished"))
        .collect();
    assert_eq!(finished.len(), 1);
    assert!(finished[0].body.contains("> look\n\nLooked."));
    // Marked done by focusing away from it: already posted.
    rig.set_focused(&tab, false);
    rig.set_state(&tab, "done", &(now + 2).to_string());
    rig.block_on(run::tab_tick(&rig.cx));
    assert_eq!(
        rig.posts()
            .iter()
            .filter(|p| p.body.contains("has finished"))
            .count(),
        1
    );
}

#[test]
fn the_dispatcher_handles_each_message_once_in_order_and_moves_the_cursor() {
    let rig = Rig::new();
    let thread = rig.ts_ago(60, 100);
    {
        let mut state = rig.cx.state.borrow_mut();
        state.cursor = Some("1791700000.000050".into());
        state.threads.insert(thread.clone(), thread.clone());
    }
    let now = util::now_secs();
    let m = |n: u32, text: &str, by_claude: bool, thread: Option<&str>| Message {
        by_claude,
        ..message(&format!("{now}.{n:06}"), text, thread)
    };
    rig.block_on(async {
        run::process_messages(
            &rig.cx,
            vec![m(2, "cleanup", false, None), m(1, "help", false, None)],
        );
        run::process_messages(
            &rig.cx,
            vec![
                m(2, "cleanup", false, None),
                m(3, "🤖 the relay's own post", true, None),
                m(4, "status", false, Some(&thread)),
            ],
        );
    });
    let firsts: Vec<String> = rig
        .posts()
        .iter()
        .map(|p| p.body.lines().next().unwrap().to_string())
        .collect();
    assert_eq!(
        firsts,
        [
            "🤖 **What I understand**",
            "🤖 No closed tabs have threads left to delete.",
            "🤖 **Status**"
        ]
    );
    let state = rig.cx.state.borrow();
    assert_eq!(
        state.cursor.as_deref(),
        Some(format!("{now}.000003").as_str()),
        "past the relay's own post too"
    );
    assert_eq!(state.threads[&thread], format!("{now}.000004"));
}

#[test]
fn a_message_that_fails_is_answered_with_the_error_and_the_next_still_runs() {
    let rig = Rig::new();
    let failed = std::cell::Cell::new(false);
    let once = std::rc::Rc::new(failed);
    let flag = once.clone();
    rig.slack.answers.borrow_mut().insert(
        "chat.postMessage".into(),
        Box::new(move |_| {
            if flag.replace(true) {
                Ok(json!({ "ok": true, "ts": "1.000001" }))
            } else {
                Err(anyhow::anyhow!("chat.postMessage: msg_too_long"))
            }
        }),
    );
    let now = util::now_secs();
    rig.block_on(async {
        run::process_messages(
            &rig.cx,
            vec![
                message(&format!("{now}.000005"), "help", Some("1791700000.000060")),
                message(&format!("{now}.000006"), "cleanup", None),
            ],
        );
    });
    let calls = rig.calls("chat.postMessage");
    assert!(
        calls[1]["text"]
            .as_str()
            .unwrap()
            .starts_with("🤖 That failed: chat.postMessage: msg_too_long")
    );
    assert_eq!(calls[1]["thread_ts"], "1791700000.000060");
    assert_eq!(
        calls.last().unwrap()["text"],
        "🤖 No closed tabs have threads left to delete."
    );
}

#[test]
fn catching_up_finds_only_the_owners_messages_since_the_cursor_and_each_threads_last_reply() {
    let rig = Rig::new();
    {
        let mut state = rig.cx.state.borrow_mut();
        state.cursor = Some("1791500000.000100".into());
        state
            .threads
            .insert("1791500000.000050".into(), "1791500000.000060".into());
    }
    rig.slack.answers.borrow_mut().insert(
        "conversations.history".into(),
        Box::new(|p| {
            assert_eq!(p["oldest"], "1791500000.000100");
            Ok(json!({ "ok": true, "messages": [
                { "user": "U1", "ts": "1791500000.000300", "text": "status", "client_msg_id": "c1" },
                { "user": "U2", "ts": "1791500000.000250", "text": "1: rm -rf ~", "client_msg_id": "c2" },
                { "user": "U1", "ts": "1791500000.000260", "text": "1: rm -rf ~", "app_id": "A0CLAUDE" },
                { "user": "B0BOT", "bot_id": "B1", "ts": "1791500000.000200", "text": "🤖 Relay listening." }
            ] }))
        }),
    );
    rig.slack.answers.borrow_mut().insert(
        "conversations.replies".into(),
        Box::new(|p| {
            assert_eq!(p["ts"], "1791500000.000050");
            Ok(json!({ "ok": true, "messages": [
                { "user": "U1", "ts": "1791500000.000050", "text": "the thread's own message", "client_msg_id": "c3" },
                { "user": "U1", "ts": "1791500000.000060", "thread_ts": "1791500000.000050", "text": "already handled", "client_msg_id": "c4" },
                { "user": "U1", "ts": "1791500000.000400", "thread_ts": "1791500000.000050", "text": "yes", "client_msg_id": "c5" }
            ] }))
        }),
    );
    let messages = rig.block_on(super::read::catch_up(&rig.cx)).unwrap();
    let got: Vec<(String, Option<String>)> = messages
        .into_iter()
        .map(|m| (m.text, m.thread_ts))
        .collect();
    assert_eq!(
        got,
        [
            ("status".into(), None),
            ("yes".into(), Some("1791500000.000050".into()))
        ]
    );
}

#[test]
fn an_image_you_attach_is_downloaded_and_its_path_typed_with_your_text() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let thread = alert(&rig, &tab, rig.index_of(&tab), "work");
    let url = "https://files.slack.com/files-pri/T1-F0SHOT/download/screen_shot.png";
    rig.slack
        .downloads
        .borrow_mut()
        .insert(url.into(), b"PNG bytes".to_vec());
    let dm = rig.cx.state.borrow().dm();
    let event = json!({
        "type": "message", "subtype": "file_share", "channel": "D1", "user": "U1", "client_msg_id": "c1",
        "ts": now_ts(40), "thread_ts": thread, "text": "what's wrong here?",
        "files": [{ "id": "F0SHOT", "name": "screen shot.png", "mimetype": "image/png", "size": 9, "url_private_download": url }],
    });
    let m = super::slack::message_from(&dm, &event, Some("T1"), |_| {}).unwrap();
    rig.block_on(handle(&rig.cx, m)).unwrap();
    let saved = rig.cx.paths.files_dir().join("F0SHOT-screen-shot.png");
    assert_eq!(std::fs::read(&saved).unwrap(), b"PNG bytes");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        rig.screen(&tab)
            .contains(&format!("what's wrong here? [Image: {}]", saved.display()))
    );
}

#[test]
fn only_images_and_videos_from_slacks_file_host_under_the_limit_are_fetched() {
    let rig = Rig::new();
    let refused =
        |file: super::slack::FileRef| match rig.block_on(super::files::download(&rig.cx, &file)) {
            Ok(super::files::Got::Refused(why)) => why,
            other => panic!("not refused: {other:?}"),
        };
    let file = |name: &str, mime: &str, size: u64, url: &str| super::slack::FileRef {
        id: "F1".into(),
        name: Some(name.into()),
        mimetype: Some(mime.into()),
        size: Some(size),
        url: Some(url.into()),
    };
    assert!(
        refused(file(
            "a.zip",
            "application/zip",
            1,
            "https://files.slack.com/a"
        ))
        .contains("isn't an image or video")
    );
    assert!(
        refused(file(
            "a.png",
            "image/png",
            30 << 20,
            "https://files.slack.com/a"
        ))
        .contains("larger than 20 MB")
    );
    assert!(
        refused(file("a.png", "image/png", 1, "https://example.com/a.png"))
            .contains("isn't on Slack's file host")
    );
}

#[test]
fn a_turns_new_images_are_shared_under_it_and_nothing_that_only_looks_like_one() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let shot = rig.root.join("shot.png");
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    png.extend_from_slice(&[0; 8]);
    std::fs::write(&shot, &png).unwrap();
    std::fs::write(rig.root.join("fake.png"), b"not an image").unwrap();
    let sent = jiff::Timestamp::from_millisecond(util::now_ms() - 60_000)
        .unwrap()
        .to_string();
    let transcript = rig.transcript(&tab, "media", &[
        json!({ "type": "user", "timestamp": sent, "message": { "content": "screenshot it" } }),
        json!({ "type": "assistant", "cwd": rig.root, "message": { "content": [{ "type": "text", "text": "Saved shot.png and fake.png." }] } }),
    ]);
    let found = super::files::turn_media(&rig.cx, &transcript);
    assert_eq!(found, [shot]);
    rig.block_on(super::files::share_media(
        &rig.cx,
        Some(&transcript),
        "1.000100",
    ));
    assert_eq!(rig.slack.uploads.borrow().len(), 1);
    let shared = rig.calls("files.completeUploadExternal");
    assert_eq!(shared[0]["thread_ts"], "1.000100");
    assert_eq!(shared[0]["files"][0]["title"], "shot.png");
}

#[test]
fn a_turns_image_at_a_path_with_spaces_named_in_backticks_is_shared() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let dir = rig.root.join("slack proposal");
    std::fs::create_dir_all(&dir).unwrap();
    let shot = dir.join("Board 50 - Slack Home.png");
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    png.extend_from_slice(&[0; 8]);
    std::fs::write(&shot, &png).unwrap();
    let sent = jiff::Timestamp::from_millisecond(util::now_ms() - 60_000)
        .unwrap()
        .to_string();
    let text = format!("Here it is: `{}`", shot.display());
    let transcript = rig.transcript(&tab, "media", &[
        json!({ "type": "user", "timestamp": sent, "message": { "content": "send me the PNG" } }),
        json!({ "type": "assistant", "cwd": rig.root, "message": { "content": [{ "type": "text", "text": text }] } }),
    ]);
    assert_eq!(super::files::turn_media(&rig.cx, &transcript), [shot]);
    rig.block_on(super::files::share_media(
        &rig.cx,
        Some(&transcript),
        "1.000100",
    ));
    let shared = rig.calls("files.completeUploadExternal");
    assert_eq!(shared[0]["thread_ts"], "1.000100");
    assert_eq!(shared[0]["files"][0]["title"], "Board 50 - Slack Home.png");
}

#[test]
fn the_watchdog_restarts_a_crashed_relay_telling_it_why_until_it_ends_cleanly() {
    let rig = Rig::new();
    let runs = rig.root.join("runs.txt");
    std::fs::write(&runs, "").unwrap();
    let log = rig.cx.paths.log_file();
    let script = format!(
        r#"n=$(($(wc -l < '{runs}') + 1)); printf '%s\n' "[$SLACK_RELAY_RESTARTED]" >> '{runs}'; if [ $n -le 2 ]; then printf '2999-01-01T00:00:0%s.000Z Error: crashed: boom %s\n' $n $n >> '{log}'; exit 1; fi; exit 0"#,
        runs = runs.display(),
        log = log.display()
    );
    let w = run::Watch {
        command: vec!["sh".into(), "-c".into(), script],
        delays: vec![10; 5],
        paths: rig.cx.paths.clone(),
    };
    let downs = std::cell::Cell::new(0);
    let code = rig.block_on(run::watch(w, |_| async { downs.set(downs.get() + 1) }));
    assert_eq!(code, 0);
    assert_eq!(downs.get(), 0);
    assert_eq!(util::read_text(&runs).unwrap(), "[]\n[boom 1]\n[boom 2]\n");
}

#[test]
fn the_watchdog_gives_up_after_five_crashes_and_says_the_relay_is_down() {
    let rig = Rig::new();
    let log = rig.cx.paths.log_file();
    let script = format!(
        r#"printf '2999-01-01T00:00:00.000Z Error: crashed: no network\n' >> '{}'; exit 1"#,
        log.display()
    );
    std::fs::write(rig.cx.paths.events_file(), "").unwrap();
    let said = std::cell::RefCell::new(Vec::new());
    let w = run::Watch {
        command: vec!["sh".into(), "-c".into(), script],
        delays: vec![10; 5],
        paths: rig.cx.paths.clone(),
    };
    let code = rig.block_on(run::watch(w, |why| async { said.borrow_mut().push(why) }));
    assert_eq!(code, 1);
    assert_eq!(*said.borrow(), ["no network"]);
    assert!(
        !rig.cx.paths.events_file().exists(),
        "nothing will read the hooks' events now"
    );
    // One that refused to start isn't started again.
    let w = run::Watch {
        command: vec!["sh".into(), "-c".into(), "exit 3".into()],
        delays: vec![10],
        paths: rig.cx.paths.clone(),
    };
    assert_eq!(
        rig.block_on(run::watch(w, |_| async { panic!("not down") })),
        3
    );
}

#[test]
fn last_error_finds_the_reason_for_the_latest_crash_in_the_log() {
    let rig = Rig::new();
    let since = util::now_ms();
    let log = rig.cx.paths.log_file();
    util::write_private(
        &log,
        format!(
            "{} Error: an older crash\n",
            super::log::iso(since - 60_000)
        ),
        true,
    )
    .unwrap();
    assert_eq!(run::last_error(&rig.cx.paths, since), None);
    util::write_private(
        &log,
        format!(
            "{} Error: crashed: TypeError: x is undefined\n",
            super::log::iso(since + 1)
        ),
        true,
    )
    .unwrap();
    assert_eq!(
        run::last_error(&rig.cx.paths, since).as_deref(),
        Some("TypeError: x is undefined")
    );
}

#[test]
fn a_restart_edits_the_welcome_back_unless_it_is_old_or_the_relay_crashed() {
    let rig = Rig::new();
    let recent = rig.ts_ago(3600, 100);
    rig.cx.state.borrow_mut().welcome = Some(recent.clone());
    let from = rig
        .block_on(run::say_listening(&rig.cx, &run::welcome_text(""), ""))
        .unwrap();
    assert!(rig.posts().is_empty());
    let update = rig.calls("chat.update").last().unwrap().clone();
    assert_eq!(update["ts"], recent.as_str());
    assert!(
        update["text"]
            .as_str()
            .unwrap()
            .starts_with("🤖 **Relay listening** since ")
    );
    assert!(
        (super::state::number(&from) - util::now_secs() as f64).abs() < 5.0,
        "nothing before now is read again"
    );
    // After a crash, a new message: it's news.
    rig.block_on(run::say_listening(
        &rig.cx,
        &run::welcome_text("exit code 1"),
        "exit code 1",
    ))
    .unwrap();
    assert!(
        rig.posts()
            .last()
            .unwrap()
            .body
            .starts_with("🤖 **Relay restarted** after a crash")
    );
    assert_eq!(
        rig.cx.state.borrow().welcome,
        Some(rig.posts().last().unwrap().ts.clone())
    );
    // A day-old welcome is too far up the DM to edit.
    rig.cx.state.borrow_mut().welcome = Some(rig.ts_ago(25 * 3600, 100));
    let kickoff = rig
        .block_on(run::say_listening(&rig.cx, &run::welcome_text(""), ""))
        .unwrap();
    assert_eq!(kickoff, rig.posts().last().unwrap().ts);
    assert!(super::lifecycle::stopped_text().starts_with("🤖 **Relay stopped** at "));
}

#[test]
fn the_welcome_offers_home_first_then_commands_answered_at_the_top_level() {
    let rig = Rig::new();
    let names = |dm: &super::state::Dm| -> Vec<String> {
        super::buttons::welcome_buttons(dm)
            .into_iter()
            .map(|b| b.text)
            .collect()
    };
    let mut dm = super::state::Dm {
        team: Some("T1".into()),
        ..Default::default()
    };
    assert_eq!(
        names(&dm),
        [
            "Status",
            "Threads",
            "Progress",
            "New tab",
            "Help",
            "Stop relay"
        ]
    );
    dm.app_id = Some("A1".into());
    assert_eq!(names(&dm)[0], "Home");
    assert_eq!(
        super::buttons::home_link(&dm),
        "slack://app?team=T1&id=A1&tab=home"
    );
    let welcome = now_ts(600);
    let press = |c: &str| super::slack::Press {
        value: json!({ "a": "do", "c": c }),
        message: Some(super::slack::Pressed {
            ts: welcome.clone(),
            thread_ts: None,
            text: "🤖 Relay listening".into(),
        }),
        ..Default::default()
    };
    rig.block_on(super::presses::handle_press(&rig.cx, press("status")))
        .unwrap();
    assert!(
        rig.posts()
            .last()
            .unwrap()
            .body
            .starts_with("🤖 **Status**")
    );
    assert_eq!(rig.posts().last().unwrap().thread, None);
    let before = rig.posts().len();
    rig.block_on(super::presses::handle_press(
        &rig.cx,
        press("new: rm -rf ~"),
    ))
    .unwrap();
    assert_eq!(rig.posts().len(), before, "only the welcome's own commands");
}

#[test]
fn an_offline_note_stays_scheduled_ahead_and_is_renewed_said_back_or_cancelled() {
    let rig = Rig::new();
    rig.block_on(super::offline::renew_offline_note(&rig.cx))
        .unwrap();
    let first = rig.calls("chat.scheduleMessage")[0].clone();
    assert_eq!(first["channel"], "D1");
    let ahead = first["post_at"].as_i64().unwrap() - util::now_secs();
    assert!((595..=605).contains(&ahead), "ten minutes out");
    assert!(
        first["text"]
            .as_str()
            .unwrap()
            .contains("**The relay is offline.** Your computer last checked in at ")
    );
    assert_eq!(
        rig.cx.state.borrow().offline_note.as_ref().unwrap().id,
        "Q1"
    );
    // Renewed: the next one first, then the old one goes, so one is always
    // there.
    rig.block_on(super::offline::renew_offline_note(&rig.cx))
        .unwrap();
    assert_eq!(
        rig.cx.state.borrow().offline_note.as_ref().unwrap().id,
        "Q2"
    );
    let deleted: Vec<Value> = rig
        .calls("chat.deleteScheduledMessage")
        .iter()
        .map(|c| c["scheduled_message_id"].clone())
        .collect();
    assert_eq!(deleted, [json!("Q1")]);
    // Notes left behind by a delete that failed go too, but not the new one.
    rig.slack.answers.borrow_mut().insert(
        "chat.scheduledMessages.list".into(),
        Box::new(|_| {
            Ok(json!({ "ok": true, "scheduled_messages": [{ "id": "QSTRAY" }, { "id": "Q3" }] }))
        }),
    );
    rig.block_on(super::offline::renew_offline_note(&rig.cx))
        .unwrap();
    assert_eq!(
        rig.cx.state.borrow().offline_note.as_ref().unwrap().id,
        "Q3"
    );
    let deleted: Vec<Value> = rig
        .calls("chat.deleteScheduledMessage")
        .iter()
        .map(|c| c["scheduled_message_id"].clone())
        .collect();
    assert_eq!(deleted, [json!("Q1"), json!("Q2"), json!("QSTRAY")]);
    rig.slack
        .answers
        .borrow_mut()
        .remove("chat.scheduledMessages.list");
    // Back after a short gap: Slack never sent it, so nothing to say.
    rig.block_on(super::offline::say_back_online(&rig.cx))
        .unwrap();
    assert!(rig.posts().is_empty());
    rig.cx.state.borrow_mut().offline_note.as_mut().unwrap().at =
        util::now_ms() - (2 * 60 + 13) * 60_000;
    rig.block_on(super::offline::say_back_online(&rig.cx))
        .unwrap();
    assert!(
        rig.posts()[0]
            .body
            .starts_with("🤖 ✅ **Back online**, after 2h 13m offline")
    );
    rig.block_on(super::offline::cancel_offline_note(&rig.cx));
    assert!(rig.cx.state.borrow().offline_note.is_none());
}

#[test]
fn notify_waits_for_the_relay_to_post_a_notice_and_says_when_it_failed() {
    let rig = Rig::new();
    let paths = rig.cx.paths.clone();
    // No relay running: refused at once, with nothing left queued.
    let refused = super::outbox::notify(&paths, "hello", None, Duration::from_secs(1)).unwrap_err();
    assert!(refused.to_string().contains("isn't running"));
    util::write_private(
        paths.lock_file(),
        format!("{} slack-relay\n", std::process::id()),
        false,
    )
    .unwrap();
    assert!(
        super::outbox::notify(&paths, "x", Some("oncall; rm"), Duration::from_secs(1))
            .unwrap_err()
            .to_string()
            .contains("--from")
    );
    let notifier = std::thread::spawn(move || {
        super::outbox::notify(
            &paths,
            "Triggered: disk usage on db-1",
            Some("oncall"),
            Duration::from_secs(10),
        )
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while rig.posts().is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        rig.block_on(super::outbox::deliver_outbox(&rig.cx));
    }
    let ts = notifier.join().unwrap().unwrap();
    let posts = rig.posts();
    assert_eq!(posts[0].ts, ts);
    assert_eq!(posts[0].thread, None);
    assert_eq!(
        posts[0].body,
        "🔔 **oncall**\nTriggered: disk usage on db-1"
    );
    assert_eq!(
        std::fs::read_dir(rig.cx.paths.outbox_dir())
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn open_on_the_home_tab_sees_a_finished_tab_as_looking_at_it_does() {
    let rig = Rig::new();
    let (done, _) = rig.claude_tab("done");
    let (waiting, _) = rig.claude_tab("waiting");
    rig.set_state(&done, "done", "100");
    rig.set_state(&waiting, "waiting", "100");
    let dm = rig.cx.state.borrow().dm();
    let open = |tab: &str| {
        let pid = rig.pid_of(tab);
        let press = json!({
            "type": "block_actions", "user": { "id": "U1", "team_id": "T1" }, "team": { "id": "T1" }, "trigger_id": "TR1",
            "container": { "type": "view", "view_id": "VHOME" }, "view": { "id": "VHOME", "type": "home" },
            "actions": [{ "value": json!({ "a": "open", "w": tab, "p": pid }).to_string() }],
        });
        let p = press_from(&dm, &press, |_| {}).unwrap();
        rig.block_on(super::presses::handle_press(&rig.cx, p))
            .unwrap();
    };
    open(&waiting);
    assert!(
        rig.mux.seen.borrow().is_empty(),
        "a tab waiting on you stays waiting"
    );
    let (reminded, _) = rig.claude_tab("reminded");
    rig.set_remind_at(&reminded, util::now_secs() - 60);
    open(&reminded);
    assert_eq!(
        *rig.mux.seen.borrow(),
        [reminded],
        "a reminder come due is seen"
    );
    rig.mux.seen.borrow_mut().clear();
    open(&done);
    assert_eq!(*rig.mux.seen.borrow(), [done]);
    let home = rig.calls("views.publish").last().unwrap().clone();
    let headers: Vec<&str> = home["view"]["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["type"] == "header")
        .filter_map(|b| b["text"]["text"].as_str())
        .collect();
    assert_eq!(headers, ["Claude tabs", "Needs you (1)", "Idle (2)"]);
}

#[test]
fn open_on_the_home_tab_opens_a_screen_for_exactly_that_tab_and_only_for_you() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    let pid = rig.pid_of(&tab);
    let dm = rig.cx.state.borrow().dm();
    let press = |view: Value, value: Value, user: &str| {
        json!({
            "type": "block_actions", "user": { "id": user, "team_id": "T1" }, "team": { "id": "T1" }, "trigger_id": "TR1",
            "container": { "type": "view", "view_id": view["id"] }, "view": view,
            "actions": [{ "value": value.to_string() }],
        })
    };
    let home = json!({ "id": "VHOME", "type": "home" });
    let modal = json!({ "id": "VTAB", "type": "modal" });
    let open = json!({ "a": "open", "w": tab, "p": pid });
    assert!(press_from(&dm, &press(home.clone(), open.clone(), "U2"), |_| {}).is_none());
    assert!(
        press_from(
            &dm,
            &press(
                home.clone(),
                json!({ "a": "screen-close", "w": tab, "p": pid }),
                "U1"
            ),
            |_| {}
        )
        .is_none()
    );
    assert!(
        press_from(
            &dm,
            &press(
                home.clone(),
                json!({ "a": "close", "w": tab, "p": pid }),
                "U1"
            ),
            |_| {}
        )
        .is_none()
    );
    assert!(press_from(&dm, &press(modal.clone(), open.clone(), "U1"), |_| {}).is_none());
    let p = press_from(&dm, &press(home, open, "U1"), |_| {}).unwrap();
    rig.block_on(super::presses::handle_press(&rig.cx, p))
        .unwrap();
    let opened = rig.calls("views.open")[0].clone();
    assert_eq!(opened["trigger_id"], "TR1");
    let screen = &opened["view"];
    assert!(
        screen["title"]["text"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{index} · "))
    );
    let meta: Value = serde_json::from_str(screen["private_metadata"].as_str().unwrap()).unwrap();
    assert_eq!(meta, json!({ "w": tab, "p": pid }));
    let actions = screen["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["type"] == "actions")
        .unwrap();
    let names: Vec<&str> = actions["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["text"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["Message", "Skill", "Progress", "Rename", "Close"],
        "no Stop for an idle tab, and no thread before it starts"
    );
    // A screen drawn for an earlier process of this tab acts on nothing.
    let stale = press_from(
        &dm,
        &press(
            modal.clone(),
            json!({ "a": "screen-close", "w": tab, "p": "999999" }),
            "U1",
        ),
        |_| {},
    )
    .unwrap();
    rig.block_on(super::presses::handle_press(&rig.cx, stale))
        .unwrap();
    assert!(rig.mux.killed.borrow().is_empty());
    let gone = rig.calls("views.update").last().unwrap().clone();
    assert!(
        gone["view"]["blocks"][0]["text"]["text"]
            .as_str()
            .unwrap()
            .contains("That tab has closed")
    );
    // Stop on an idle tab says so on the screen.
    let stop = press_from(
        &dm,
        &press(
            modal.clone(),
            json!({ "a": "screen-stop", "w": tab, "p": pid }),
            "U1",
        ),
        |_| {},
    )
    .unwrap();
    rig.block_on(super::presses::handle_press(&rig.cx, stop))
        .unwrap();
    let said = rig.calls("views.update").last().unwrap().clone();
    assert_eq!(said["view_id"], "VTAB");
    let last = said["view"]["blocks"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert!(
        last["text"]["text"]
            .as_str()
            .unwrap()
            .contains("isn't in the middle of anything")
    );
    // Close from the screen closes the tab and says so there.
    let close = press_from(
        &dm,
        &press(
            modal,
            json!({ "a": "screen-close", "w": tab, "p": pid }),
            "U1",
        ),
        |_| {},
    )
    .unwrap();
    rig.block_on(super::presses::handle_press(&rig.cx, close))
        .unwrap();
    assert_eq!(*rig.mux.killed.borrow(), std::slice::from_ref(&tab));
    let closed = rig.calls("views.update").last().unwrap().clone();
    assert!(
        closed["view"]["blocks"][0]["text"]["text"]
            .as_str()
            .unwrap()
            .starts_with(&format!("Closed tab {index}, "))
    );
}

#[test]
fn rename_on_a_tabs_screen_asks_for_the_name_and_save_renames_that_tab() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    let pid = rig.pid_of(&tab);
    super::mux::Mux::set_option(&rig.mux, &tab, "@tab_name", Some("old name")).unwrap();
    let dm = rig.cx.state.borrow().dm();
    let press = json!({
        "type": "block_actions", "user": { "id": "U1", "team_id": "T1" }, "team": { "id": "T1" }, "trigger_id": "TR2",
        "container": { "type": "view", "view_id": "VTAB" }, "view": { "id": "VTAB", "type": "modal" },
        "actions": [{ "value": json!({ "a": "screen-rename", "w": tab, "p": pid }).to_string() }],
    });
    let p = press_from(&dm, &press, |_| {}).unwrap();
    rig.block_on(super::presses::handle_press(&rig.cx, p))
        .unwrap();
    let pushed = rig.calls("views.push")[0].clone();
    assert_eq!(pushed["trigger_id"], "TR2");
    assert_eq!(pushed["view"]["callback_id"], "rename");
    assert_eq!(
        pushed["view"]["blocks"][0]["element"]["initial_value"],
        "old name"
    );
    let save = |name: &str, user: &str| {
        let mut view = pushed["view"].clone();
        view["root_view_id"] = json!("VTAB");
        view["state"] = json!({ "values": { "name": { "name": { "value": name } } } });
        json!({ "type": "view_submission", "user": { "id": user, "team_id": "T1" }, "team": { "id": "T1" }, "view": view })
    };
    assert!(
        submission_from(&dm, &save("hijacked", "U2"), |_| {}).is_none(),
        "only yours"
    );
    let form = submission_from(&dm, &save("  new   name ", "U1"), |_| {}).unwrap();
    rig.block_on(super::screens::handle_submission(&rig.cx, &form))
        .unwrap();
    assert_eq!(tabs::tab(&rig.cx, &tab).unwrap().own_name, "new name");
    let redrawn = rig.calls("views.update").last().unwrap().clone();
    assert_eq!(redrawn["view_id"], "VTAB", "the tab's screen under it");
    let last = redrawn["view"]["blocks"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert!(
        last["text"]["text"]
            .as_str()
            .unwrap()
            .contains(&format!("Renamed *tab {index}* to *new name*"))
    );
    let form = submission_from(&dm, &save("", "U1"), |_| {}).unwrap();
    rig.block_on(super::screens::handle_submission(&rig.cx, &form))
        .unwrap();
    assert_eq!(
        tabs::tab(&rig.cx, &tab).unwrap().own_name,
        "",
        "its own name back"
    );
}

#[test]
fn rename_in_a_tabs_thread_names_it_and_clears_it() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    let thread = alert(&rig, &tab, index, "work");
    rig.block_on(handle(
        &rig.cx,
        message(&now_ts(30), "rename   The   login fix", Some(&thread)),
    ))
    .unwrap();
    assert_eq!(tabs::tab(&rig.cx, &tab).unwrap().own_name, "The login fix");
    assert_eq!(
        rig.posts()[0].body,
        format!("🤖 Renamed **tab {index}** to **The login fix**.")
    );
    rig.block_on(handle(
        &rig.cx,
        message(&now_ts(31), "rename", Some(&thread)),
    ))
    .unwrap();
    assert_eq!(tabs::tab(&rig.cx, &tab).unwrap().own_name, "");
    assert_eq!(
        rig.posts()[1].body,
        format!("🤖 **Tab {index}** has its own name again.")
    );
}

#[test]
fn message_and_new_tab_forms_type_into_the_tab_or_start_one_from_you_only() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let pid = rig.pid_of(&tab);
    let dm = rig.cx.state.borrow().dm();
    let form = |callback: &str, values: Value, meta: Value| {
        json!({
            "type": "view_submission", "user": { "id": "U1", "team_id": "T1" },
            "view": { "callback_id": callback, "root_view_id": "VTAB", "private_metadata": meta.to_string(), "state": { "values": values } }
        })
    };
    let message = submission_from(
        &dm,
        &form(
            "message",
            json!({ "text": { "text": { "value": "run the tests" } } }),
            json!({ "w": tab, "p": pid }),
        ),
        |_| {},
    )
    .unwrap();
    rig.block_on(super::screens::handle_submission(&rig.cx, &message))
        .unwrap();
    assert!(rig.screen(&tab).contains("run the tests"));
    let home = rig.cx.state.borrow().homes[&tab].ts.clone();
    assert!(
        rig.posts()
            .iter()
            .any(|p| p.thread.as_deref() == Some(home.as_str())
                && p.body.contains("_From the Home tab:_ run the tests"))
    );
    assert_eq!(
        rig.cx.state.borrow().following[&tab].thread.as_deref(),
        Some(home.as_str())
    );
    let new = submission_from(
        &dm,
        &form(
            "new",
            json!({ "prompt": { "prompt": { "value": "look around" } } }),
            json!({}),
        ),
        |_| {},
    )
    .unwrap();
    rig.block_on(super::screens::handle_submission(&rig.cx, &new))
        .unwrap();
    let windows = rig.mux.windows.borrow();
    assert!(
        windows.last().unwrap().1[0]
            .screen
            .starts_with("claude 'look around'")
    );
}

#[test]
fn a_closed_tabs_thread_is_deleted_on_request_and_a_live_tabs_cant_be() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let live = "1791500000.000100";
    {
        let mut state = rig.cx.state.borrow_mut();
        state.dm.as_mut().unwrap().bot = Some("B1".into());
        state.homes.insert(
            tab.clone(),
            Home {
                ts: live.into(),
                pid: rig.pid_of(&tab),
                label: "1, work".into(),
                ..Home::default()
            },
        );
        state.threads.insert(live.into(), live.into());
        state.homes.insert(
            "@99".into(),
            Home {
                ts: "1791500000.000200".into(),
                pid: "1".into(),
                label: "2, gone".into(),
                ..Home::default()
            },
        );
    }
    rig.slack.answers.borrow_mut().insert(
        "conversations.replies".into(),
        Box::new(|p| {
            Ok(json!({ "ok": true, "messages": [
                { "ts": p["ts"], "bot_id": "B1" },
                { "ts": "1791500000.000300", "bot_id": "B1", "files": [{ "id": "F7" }] },
                { "ts": "1791500000.000400", "user": "U1" }
            ] }))
        }),
    );
    // The tab @99 has closed: its thread is marked so.
    let tabs_now = tabs::claude_tabs(&rig.cx);
    rig.block_on(alerts::alert_tabs(&rig.cx, &tabs_now, &[]));
    assert_eq!(
        super::cleanup::closed_threads(&rig.cx),
        ["1791500000.000200"]
    );
    let done = rig.block_on(super::cleanup::delete_threads(
        &rig.cx,
        &["1791500000.000200".into(), live.into()],
    ));
    assert_eq!(
        done,
        super::cleanup::Deleted {
            deleted: 1,
            yours: 1
        }
    );
    let deleted: Vec<Value> = rig
        .calls("chat.delete")
        .iter()
        .map(|c| c["ts"].clone())
        .collect();
    assert_eq!(
        deleted,
        [json!("1791500000.000300"), json!("1791500000.000200")],
        "replies first, the header last, never the live tab's"
    );
    assert_eq!(rig.calls("files.delete")[0]["file"], "F7");
    assert!(super::cleanup::closed_threads(&rig.cx).is_empty());
}

#[test]
fn your_replies_are_counted_in_a_thread_that_then_fails_to_go() {
    let rig = Rig::new();
    let closed = "1791500000.000200";
    {
        let mut state = rig.cx.state.borrow_mut();
        state.dm.as_mut().unwrap().bot = Some("B1".into());
        state.closed_homes.insert(
            closed.into(),
            super::state::ClosedHome {
                label: "2, gone".into(),
                at: 1,
            },
        );
    }
    rig.slack.answers.borrow_mut().insert(
        "conversations.replies".into(),
        Box::new(|p| {
            Ok(json!({ "ok": true, "messages": [
                { "ts": p["ts"], "bot_id": "B1" },
                { "ts": "1791500000.000400", "user": "U1" }
            ] }))
        }),
    );
    rig.slack.answers.borrow_mut().insert(
        "chat.delete".into(),
        Box::new(|_| Err(anyhow::anyhow!("chat.delete: cant_delete_message"))),
    );
    let done = rig.block_on(super::cleanup::delete_threads(&rig.cx, &[closed.into()]));
    assert_eq!(
        done,
        super::cleanup::Deleted {
            deleted: 0,
            yours: 1
        }
    );
    assert_eq!(super::cleanup::closed_threads(&rig.cx), [closed]);
}

#[test]
fn a_tab_keeps_the_claude_pane_it_was_first_seen_with() {
    let rig = Rig::new();
    let (tab, first) = rig.claude_tab("work");
    // A second Claude opened beside it.
    {
        let mut windows = rig.mux.windows.borrow_mut();
        let (_, panes) = windows.iter_mut().find(|(w, _)| w.id == tab).unwrap();
        let mut other = panes[0].clone();
        other.info.id = "%77".into();
        other.info.pid = "77".into();
        panes.insert(0, other);
    }
    let seen = tabs::claude_tabs(&rig.cx);
    assert_eq!(
        seen[0].pane, "%77",
        "the first Claude pane seen is recorded"
    );
    // Its first pane stops running Claude: the other stands in, unrecorded.
    {
        let mut windows = rig.mux.windows.borrow_mut();
        let (_, panes) = windows.iter_mut().find(|(w, _)| w.id == tab).unwrap();
        panes[0].info.claude = false;
    }
    assert_eq!(tabs::claude_tabs(&rig.cx)[0].pane, first);
    {
        let mut windows = rig.mux.windows.borrow_mut();
        let (w, panes) = windows.iter_mut().find(|(w, _)| w.id == tab).unwrap();
        assert_eq!(
            w.recorded.as_deref(),
            Some("%77"),
            "not recorded while the first is still open"
        );
        panes[0].info.claude = true;
    }
    assert_eq!(
        tabs::claude_tabs(&rig.cx)[0].pane,
        "%77",
        "back to it once it runs Claude again"
    );
}

#[test]
fn the_relay_never_reports_on_its_own_tab() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("relay");
    super::mux::Mux::set_option(&rig.mux, &tab, "@slack_relay", Some("1")).unwrap();
    assert!(tabs::claude_tabs(&rig.cx).is_empty());
}

#[test]
fn stop_in_the_dm_marks_the_welcome_stopped_and_ends_the_run() {
    let rig = Rig::new();
    rig.cx.state.borrow_mut().welcome = Some(rig.ts_ago(60, 1));
    rig.block_on(handle(&rig.cx, message(&now_ts(50), "stop", None)))
        .unwrap();
    assert!(rig.exited.get());
    assert!(rig.posts()[0].body.contains("**Stopped listening.**"));
    assert!(
        rig.calls("chat.update")[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("🤖 **Relay stopped** at ")
    );
    let views = rig.calls("views.publish");
    assert!(
        views[0]["view"]["blocks"][1]["text"]["text"]
            .as_str()
            .unwrap()
            .starts_with("⚪ The relay stopped")
    );
}

#[test]
fn an_alert_whose_thread_couldnt_be_started_goes_out_on_the_next_tick() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_state(&tab, "waiting", "100");
    rig.set_screen(&tab, "Allow this edit? 1. Yes 2. No\n");
    let run = |rig: &Rig| {
        let tabs = tabs::claude_tabs(&rig.cx);
        rig.block_on(alerts::alert_tabs(&rig.cx, &tabs, &[]));
    };
    rig.slack.answers.borrow_mut().insert(
        "chat.postMessage".into(),
        Box::new(|_| Err(anyhow::anyhow!("chat.postMessage: ratelimited"))),
    );
    run(&rig);
    assert!(
        !rig.cx.state.borrow().reported.contains_key(&tab),
        "not reported, so the next tick tries again"
    );
    assert!(rig.cx.state.borrow().homes.is_empty());
    rig.slack.answers.borrow_mut().clear();
    rig.slack.calls.borrow_mut().clear();
    run(&rig);
    let posts = rig.posts();
    assert!(posts[0].body.contains("This is its thread"));
    assert!(posts[1].body.contains("is **waiting on you**"));
    assert_eq!(
        posts[1].thread.as_ref(),
        Some(&posts[0].ts),
        "in the thread, never at the top level"
    );
}

#[test]
fn an_alert_whose_line_in_the_main_dm_failed_while_away_is_sent_again() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_state(&tab, "waiting", "100");
    rig.set_screen(&tab, "Allow this edit? 1. Yes 2. No\n");
    let run = |rig: &Rig| {
        let tabs = tabs::claude_tabs(&rig.cx);
        rig.block_on(alerts::alert_tabs(&rig.cx, &tabs, &[]));
    };
    // Only the line also sent to the main DM fails.
    let posted = rig.slack.clone();
    rig.slack.answers.borrow_mut().insert(
        "chat.postMessage".into(),
        Box::new(move |params| {
            if params["reply_broadcast"] == true {
                return Err(anyhow::anyhow!("chat.postMessage: fatal_error"));
            }
            let n = posted.posted_ts.borrow().len();
            let ts = format!("{}.{n:06}", util::now_secs());
            posted.posted_ts.borrow_mut().push(ts.clone());
            Ok(json!({ "ok": true, "ts": ts }))
        }),
    );
    run(&rig);
    assert!(!rig.cx.state.borrow().reported.contains_key(&tab));
    rig.slack.answers.borrow_mut().clear();
    rig.slack.calls.borrow_mut().clear();
    rig.slack.posted_ts.borrow_mut().clear();
    run(&rig);
    let lines: Vec<_> = rig.posts().into_iter().filter(|p| p.broadcast).collect();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].body.ends_with("is **waiting on you**."));
    assert!(rig.cx.state.borrow().reported.contains_key(&tab));
}

/// A finished turn's hook event for a transcript.
fn done_event(time: i64, transcript: &std::path::Path, last: &str) -> super::followups::Event {
    super::followups::Event {
        event: "done".into(),
        time,
        transcript: Some(transcript.to_string_lossy().into_owned()),
        last: Some(last.into()),
        ..Default::default()
    }
}

#[test]
fn a_turn_says_how_full_its_context_is_and_past_90_its_thread_is_told_once() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_focused(&tab, true);
    rig.at_computer();
    let transcript = rig.transcript(
        &tab,
        "nearly-full",
        &[json!({ "type": "user", "message": { "content": "check the build" } })],
    );
    let context = rig.cx.paths.context_dirs[0].join("nearly-full");
    std::fs::create_dir_all(context.parent().unwrap()).unwrap();
    let now = util::now_secs();
    let turn = |time: i64, last: &str| {
        let tabs = tabs::claude_tabs(&rig.cx);
        rig.block_on(alerts::alert_tabs(
            &rig.cx,
            &tabs,
            &[done_event(time, &transcript, last)],
        ));
    };
    let warnings = |rig: &Rig| {
        rig.posts()
            .into_iter()
            .filter(|p| p.body.contains("of its context"))
            .collect::<Vec<_>>()
    };
    std::fs::write(&context, "92\tOpus 5.5 (1M context)").unwrap();
    turn(now, "The build passes.");
    let finished = rig
        .posts()
        .into_iter()
        .find(|p| p.body.contains("has finished"))
        .unwrap();
    assert!(
        finished
            .body
            .ends_with("_92% context used · Opus 5.5 (1M context)_"),
        "{}",
        finished.body
    );
    let warned = warnings(&rig);
    assert_eq!(warned.len(), 1);
    assert!(warned[0].body.contains("has used **92%** of its context"));
    let home = rig.cx.state.borrow().homes[&tab].ts.clone();
    assert_eq!(warned[0].thread.as_deref(), Some(home.as_str()));
    assert_eq!(warned[0].buttons[0]["text"]["text"], "Compact");
    assert!(
        warned[0].buttons[0]["value"]
            .as_str()
            .unwrap()
            .contains(r#""skill":"compact""#)
    );
    std::fs::write(&context, "95\tOpus 5.5 (1M context)").unwrap();
    turn(now + 1, "Still passing.");
    assert_eq!(warnings(&rig).len(), 1, "not again while it stays above");
    std::fs::write(&context, "30\tOpus 5.5 (1M context)").unwrap();
    turn(now + 2, "Compacted.");
    std::fs::write(&context, "91\tOpus 5.5 (1M context)").unwrap();
    turn(now + 3, "Full again.");
    assert_eq!(
        warnings(&rig).len(),
        2,
        "again once it has dropped below and passed it again"
    );
}

#[test]
fn a_turn_you_asked_for_from_slack_is_posted_once_not_also_alerted() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    let transcript = rig.transcript(&tab, "asked", &[
        json!({ "type": "user", "message": { "content": "how does it work" } }),
        json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Like this." }] } }),
    ]);
    let now = util::now_secs();
    // Working when the pass begins, as the look before it saw it.
    rig.set_state(&tab, "working", &(now - 30).to_string());
    let home = "1791500000.000300";
    {
        let mut state = rig.cx.state.borrow_mut();
        state.threads.insert(home.into(), home.into());
        state.homes.insert(
            tab.clone(),
            Home {
                ts: home.into(),
                pid: "1".into(),
                ..Home::default()
            },
        );
        state.following.insert(
            tab.clone(),
            Follow {
                taken: true,
                broadcast: true,
                ..follow(Some(home), index, "how does it work")
            },
        );
    }
    let before = tabs::claude_tabs(&rig.cx);
    // Then the turn ends: the hook's event, and the tab marked done.
    let mut events = vec![done_event(now, &transcript, "Like this.")];
    rig.set_state(&tab, "done", &now.to_string());
    rig.block_on(follow_ups(&rig.cx, &before, &mut events));
    for _ in 0..2 {
        let tabs = tabs::claude_tabs(&rig.cx);
        rig.block_on(alerts::alert_tabs(&rig.cx, &tabs, &events));
    }
    let posts = rig.posts();
    assert_eq!(
        posts.len(),
        1,
        "{:?}",
        posts
            .iter()
            .map(|p| p.body.lines().next())
            .collect::<Vec<_>>()
    );
    assert!(posts[0].body.contains("is done:"));
}

#[test]
fn a_turn_in_a_tab_you_arent_looking_at_is_posted_once() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let transcript = rig.transcript(
        &tab,
        "elsewhere",
        &[json!({ "type": "user", "message": { "content": "run it" } })],
    );
    let time = util::now_secs();
    // The hook's event lands while the tab still says working...
    rig.set_state(&tab, "working", "");
    let tabs_now = tabs::claude_tabs(&rig.cx);
    rig.block_on(alerts::alert_tabs(
        &rig.cx,
        &tabs_now,
        &[done_event(time, &transcript, "Ran it.")],
    ));
    // ...then the tab is marked done.
    rig.set_state(&tab, "done", &time.to_string());
    let tabs_now = tabs::claude_tabs(&rig.cx);
    rig.block_on(alerts::alert_tabs(&rig.cx, &tabs_now, &[]));
    assert_eq!(
        rig.posts()
            .iter()
            .filter(|p| p.body.contains("has finished"))
            .count(),
        1
    );
}

#[test]
fn the_answer_to_a_request_from_the_main_dm_goes_to_the_main_dm_too() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    let transcript = rig.transcript(&tab, "homed", &[
        json!({ "type": "user", "message": { "content": "ship it" } }),
        json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Shipped." }] } }),
    ]);
    rig.events(&[
        json!({ "event": "done", "time": util::now_secs(), "transcript_path": transcript }),
    ]);
    let home = rig.ts_ago(60, 100);
    {
        let mut state = rig.cx.state.borrow_mut();
        state.threads.insert(home.clone(), home.clone());
        state.homes.insert(
            tab.clone(),
            Home {
                ts: home.clone(),
                pid: "1".into(),
                ..Home::default()
            },
        );
        state.following.insert(
            tab.clone(),
            Follow {
                broadcast: true,
                ..follow(Some(&home), index, "ship it")
            },
        );
    }
    ticks(&rig);
    let posts = rig.posts();
    assert_eq!(posts.len(), 1, "no extra line");
    assert_eq!(
        posts[0].body,
        format!("🤖 tab {index}, work is done:\n> ship it\n\nShipped.")
    );
    assert_eq!(posts[0].thread.as_deref(), Some(home.as_str()));
    assert!(posts[0].broadcast);
}

#[test]
fn a_waiting_tabs_notice_carries_its_options_and_one_answer_clears_the_rest() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_screen(&tab, "Allow rm -rf build?\n❯ 1. Yes\n  2. No\n");
    rig.set_state(&tab, "waiting", "1791500200");
    let run = |rig: &Rig| {
        let tabs = tabs::claude_tabs(&rig.cx);
        rig.block_on(alerts::alert_tabs(&rig.cx, &tabs, &[]));
    };
    run(&rig);
    let posts = rig.posts();
    let alert_post = posts
        .iter()
        .find(|p| p.body.contains("is **waiting on you**. Reply here"))
        .unwrap()
        .clone();
    let notice = posts.iter().find(|p| p.broadcast).unwrap().clone();
    let names: Vec<&str> = notice
        .buttons
        .iter()
        .map(|b| b["text"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["1. Yes", "2. No"],
        "the notice in the main DM has the buttons too"
    );
    // Tapped on the notice: typed, and the alert in the thread loses its
    // buttons.
    let yes: Value = serde_json::from_str(notice.buttons[0]["value"].as_str().unwrap()).unwrap();
    let press = super::slack::Press {
        value: yes,
        message: Some(super::slack::Pressed {
            ts: notice.ts.clone(),
            thread_ts: notice.thread.clone(),
            text: notice.body.clone(),
        }),
        ..Default::default()
    };
    rig.block_on(super::presses::handle_press(&rig.cx, press))
        .unwrap();
    assert!(rig.screen(&tab).ends_with("\n1"));
    let updates = rig.calls("chat.update");
    let cleared = updates
        .iter()
        .rfind(|u| u["ts"] == alert_post.ts.as_str())
        .unwrap();
    assert!(
        cleared["text"]
            .as_str()
            .unwrap()
            .ends_with("_Answered: 1. Yes_")
    );
    assert_eq!(
        cleared["blocks"].as_array().unwrap().len(),
        1,
        "no buttons left on the alert"
    );
    let chose = updates
        .iter()
        .find(|u| u["ts"] == notice.ts.as_str())
        .unwrap();
    assert!(
        chose["text"]
            .as_str()
            .unwrap()
            .ends_with("_Chose **1. Yes**._")
    );
    // The next prompt is answered at the computer: its buttons go.
    rig.set_screen(&tab, "Allow rm -rf dist?\n❯ 1. Yes\n  2. No\n");
    rig.set_state(&tab, "waiting", "1791500300");
    rig.cx.state.borrow_mut().following.clear();
    run(&rig);
    let second = rig.posts().into_iter().rfind(|p| !p.broadcast).unwrap().ts;
    rig.set_state(&tab, "working", "1791500310");
    run(&rig);
    let gone = rig
        .calls("chat.update")
        .into_iter()
        .rfind(|u| u["ts"] == second.as_str())
        .unwrap();
    assert!(
        gone["text"]
            .as_str()
            .unwrap()
            .ends_with("_This prompt is no longer showing._")
    );
    assert_eq!(gone["blocks"].as_array().unwrap().len(), 1);
    assert!(!rig.cx.state.borrow().choices.contains_key(&tab));
}

#[test]
fn a_waiting_tabs_options_are_on_its_home_tab_row_and_a_tap_there_answers_it() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_screen(&tab, "Allow rm -rf build?\n❯ 1. Yes\n  2. No\n");
    rig.set_state(&tab, "waiting", "1791500200");
    rig.block_on(refresh_dashboard(
        &rig.cx,
        Refresh {
            quick: true,
            opened: true,
            ..Refresh::default()
        },
    ))
    .unwrap();
    let views = rig.calls("views.publish");
    let row = views.last().unwrap()["view"]["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| {
            b["type"] == "actions"
                && serde_json::from_str::<Value>(b["elements"][0]["value"].as_str().unwrap_or(""))
                    .is_ok_and(|v| v["w"] == tab.as_str())
        })
        .unwrap()
        .clone();
    let names: Vec<&str> = row["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["text"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Open", "1. Yes", "2. No"]);
    let yes: Value = serde_json::from_str(row["elements"][1]["value"].as_str().unwrap()).unwrap();
    assert_eq!(yes["a"], "home-opt");
    let dm = rig.cx.state.borrow().dm();
    let press = |value: &Value| {
        let raw = json!({
            "type": "block_actions", "user": { "id": "U1", "team_id": "T1" }, "team": { "id": "T1" }, "trigger_id": "TR1",
            "container": { "type": "view", "view_id": "VHOME" }, "view": { "id": "VHOME", "type": "home" },
            "actions": [{ "value": value.to_string() }],
        });
        press_from(&dm, &raw, |_| {}).unwrap()
    };
    // For an earlier wait: nothing pressed, and the Home tab drawn again.
    let drawn = rig.calls("views.publish").len();
    let mut earlier = yes.clone();
    earlier["s"] = json!("1791500100");
    rig.block_on(super::presses::handle_press(&rig.cx, press(&earlier)))
        .unwrap();
    assert!(!rig.screen(&tab).ends_with('1'));
    assert_eq!(rig.calls("views.publish").len(), drawn + 1);
    rig.block_on(super::presses::handle_press(&rig.cx, press(&yes)))
        .unwrap();
    assert!(rig.screen(&tab).ends_with("\n1"));
    let home = rig.cx.state.borrow().homes[&tab].ts.clone();
    assert!(
        rig.posts()
            .iter()
            .any(|p| p.thread.as_deref() == Some(home.as_str())
                && p.body == "🤖 _From the Home tab:_ chose **1. Yes**")
    );
}

#[test]
fn a_tabs_thread_header_follows_its_name_and_state_and_says_when_it_has_ended() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    let home = "1791500000.000300";
    let t = tabs::tab(&rig.cx, &tab).unwrap();
    {
        let mut state = rig.cx.state.borrow_mut();
        state.threads.insert(home.into(), home.into());
        state.homes.insert(
            tab.clone(),
            Home {
                ts: home.into(),
                pid: t.pid.clone(),
                label: format!("{index}, work"),
                shown: super::homes::header_key(&t),
            },
        );
    }
    let refresh = |rig: &Rig| {
        let tabs = tabs::claude_tabs(&rig.cx);
        rig.block_on(super::homes::refresh_homes(&rig.cx, &tabs));
    };
    refresh(&rig);
    assert!(rig.calls("chat.update").is_empty(), "unchanged");
    rig.mux
        .windows
        .borrow_mut()
        .iter_mut()
        .find(|(w, _)| w.id == tab)
        .unwrap()
        .0
        .name = "ABC-214: Make the thing".into();
    refresh(&rig);
    let updated = rig.calls("chat.update")[0].clone();
    assert_eq!(updated["ts"], home);
    assert!(updated["text"].as_str().unwrap().starts_with(&format!(
        "🤖 **Tab {index}, ABC-214: Make the thing**\n⚪ Idle\n\nThis is its thread"
    )));
    let names: Vec<&str> = updated["blocks"][1]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["text"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Progress", "Skill", "Rename", "Close"]);
    // A new state: the header says so, and since when.
    rig.set_state(&tab, "waiting", &util::now_secs().to_string());
    refresh(&rig);
    let text = rig.calls("chat.update").last().unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(text.contains("\n🔴 **Waiting on you** since "), "{text}");
    // The tab now runs another session: the old thread is marked closed.
    rig.cx.state.borrow_mut().homes.get_mut(&tab).unwrap().pid = "999999".into();
    refresh(&rig);
    let closed = rig.calls("chat.update").last().unwrap().clone();
    assert!(closed["text"].as_str().unwrap().starts_with("🤖 ~~Tab "));
    assert!(closed["text"].as_str().unwrap().contains("⚫ Closed at "));
    let buttons = closed["blocks"][1]["elements"].as_array().unwrap();
    let names: Vec<&str> = buttons
        .iter()
        .map(|b| b["text"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Delete thread"], "only the offer to delete it");
    assert!(buttons[0].get("confirm").is_some(), "which Slack confirms");
    let state = rig.cx.state.borrow();
    assert!(!state.homes.contains_key(&tab));
    assert!(
        state.closed_homes.contains_key(home),
        "recorded as a thread that may be deleted"
    );
}

#[test]
fn a_threads_link_opens_at_its_newest_message_yours_or_the_relays() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let home = "1791500000.000300";
    {
        let mut state = rig.cx.state.borrow_mut();
        state.dm.as_mut().unwrap().url = Some("https://example.slack.com/".into());
        state.threads.insert(home.into(), home.into());
        state.homes.insert(
            tab.clone(),
            Home {
                ts: home.into(),
                pid: "1".into(),
                ..Home::default()
            },
        );
    }
    let link_to = |ts: &str| {
        format!(
            "[open](https://example.slack.com/archives/D1/p{}?thread_ts={home}&cid=D1)",
            ts.replace('.', "")
        )
    };
    // Nothing newer known: the thread's own first message.
    assert_eq!(
        super::homes::thread_link(&rig.cx, home, "open"),
        link_to(home)
    );
    // The relay posts in it, in two parts: the last part is the newest.
    let text: Vec<String> = (0..600).map(|i| format!("line {i}")).collect();
    rig.block_on(super::post::post(
        &rig.cx,
        &text.join("\n"),
        Some(home),
        super::post::PostOpts::default(),
    ))
    .unwrap();
    let last = rig.posts().last().unwrap().ts.clone();
    assert!(rig.posts().len() >= 2);
    assert_eq!(
        super::homes::thread_link(&rig.cx, home, "open"),
        link_to(&last)
    );
    // Then you reply there, and that is the newest.
    let secs: i64 = last.split('.').next().unwrap().parse().unwrap();
    let reply = format!("{}.000100", secs + 5);
    rig.block_on(async {
        run::process_messages(&rig.cx, vec![message(&reply, "status", Some(home))]);
    });
    let newest = rig.posts().last().unwrap().ts.clone();
    let newest = if newest.parse::<f64>().unwrap() > reply.parse::<f64>().unwrap() {
        newest
    } else {
        reply.clone()
    };
    assert_eq!(
        super::homes::thread_link(&rig.cx, home, "open"),
        link_to(&newest)
    );
    assert!(
        rig.cx.state.borrow().latest[home].parse::<f64>().unwrap() >= reply.parse::<f64>().unwrap(),
        "your reply counts"
    );
}

#[test]
fn no_stuck_notes_in_the_first_ten_minutes_after_the_computer_wakes() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    rig.set_state(&tab, "working", "");
    // Nothing since the computer went to sleep, hours ago.
    let slept = jiff::Timestamp::from_millisecond(util::now_ms() - 5 * 60 * 60_000)
        .unwrap()
        .to_string();
    rig.transcript(
        &tab,
        "slept",
        &[json!({ "type": "user", "timestamp": slept, "message": { "content": "long job" } })],
    );
    let run = |rig: &Rig| {
        let tabs = tabs::claude_tabs(&rig.cx);
        rig.block_on(super::stuck::stuck_tabs(&rig.cx, &tabs));
    };
    rig.cx.life.borrow_mut().woke_at = util::now_ms() - 60_000;
    run(&rig);
    assert!(rig.posts().is_empty(), "it just woke up");
    rig.cx.life.borrow_mut().woke_at = util::now_ms() - super::stuck::STUCK_MS - 1_000;
    run(&rig);
    assert!(
        rig.posts().iter().any(|p| p.body.contains("may be stuck")),
        "ten minutes after waking, it is"
    );
}

#[test]
fn the_watchdog_doesnt_restart_a_relay_that_refused_to_start() {
    let rig = Rig::new();
    let w = run::Watch {
        command: vec!["sh".into(), "-c".into(), "exit 3".into()],
        delays: vec![10],
        paths: rig.cx.paths.clone(),
    };
    let downs = std::cell::Cell::new(0);
    let code = rig.block_on(run::watch(w, |_| async { downs.set(downs.get() + 1) }));
    assert_eq!(code, 3);
    assert_eq!(downs.get(), 0);
}

#[test]
fn a_tabs_screen_you_have_closed_is_forgotten_without_an_error_in_the_log() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let pid = rig.pid_of(&tab);
    rig.block_on(super::presses::handle_press(
        &rig.cx,
        super::slack::Press {
            value: json!({ "a": "open", "w": tab, "p": pid }),
            view: Some(super::slack::ViewRef {
                id: "VHOME".into(),
                ..Default::default()
            }),
            trigger_id: Some("TR0".into()),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(super::screens::open_screens(&rig.cx), 1);
    super::mux::Mux::kill_tab(&rig.mux, &tab).unwrap();
    rig.slack.answers.borrow_mut().insert(
        "views.update".into(),
        Box::new(|_| Err(anyhow::anyhow!("views.update: not_found"))),
    );
    rig.block_on(super::screens::refresh_screens(&rig.cx, &[]));
    assert_eq!(super::screens::open_screens(&rig.cx), 0);
    let logged = util::read_text(rig.cx.paths.log_file()).unwrap_or_default();
    assert!(!logged.contains("screen update failed"), "{logged}");
}

#[test]
fn answering_one_of_several_questions_posts_the_next_and_ticks_and_submit_press_the_right_keys() {
    let rig = Rig::new();
    let (tab, pane) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    rig.set_screen(
        &tab,
        "←  ☐ Color  ☐ Toppings  ✔ Submit  →\nColor?\n❯ 1. Red\n  2. Blue\n  3. Type something.\n────────────\n  4. Chat about this\n",
    );
    rig.set_state(&tab, "waiting", "1791500300");
    let linked = rig.ts_ago(30, 500);
    rig.cx.state.borrow_mut().alerts.insert(
        linked.clone(),
        Target {
            window: tab.clone(),
            index,
            name: "work".into(),
            thread: None,
        },
    );
    let last = |rig: &Rig| rig.posts().last().unwrap().clone();
    // Red is picked, and Claude Code shows the second question in the same
    // wait, drawn before the relay's first look after its settle.
    let toppings = "←  ☒ Color  ☐ Toppings  ✔ Submit  →\nToppings?\n❯ 1. [ ] Cheese\n  2. [ ] Olives\n  3. [ ] Type something\n     Submit\n────────────\n  4. Chat about this\n";
    rig.block_on(async {
        let mux = rig.mux.clone();
        let id = tab.clone();
        tokio::task::spawn_local(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let mut windows = mux.windows.borrow_mut();
            let (_, panes) = windows.iter_mut().find(|(w, _)| w.id == id).unwrap();
            panes[0].screen = toppings.into();
        });
        handle(&rig.cx, message(&now_ts(710), "1", Some(&linked))).await
    })
    .unwrap();
    let next = last(&rig);
    assert!(
        next.body.starts_with(&format!(
            "🤖 Chose 1. Red in tab {index}, work. It's asking next:\n\n**Toppings?**\n1. ☐ Cheese"
        )),
        "{}",
        next.body
    );
    let names: Vec<&str> = next
        .buttons
        .iter()
        .map(|b| b["text"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "1. ☐ Cheese",
            "2. ☐ Olives",
            "4. Chat about this",
            "Submit choices"
        ]
    );
    assert!(
        !rig.cx.state.borrow().following.contains_key(&tab),
        "still waiting on you, so not followed yet"
    );
    // Two ticks at once, then your own answer typed into the free row,
    // reached with the arrow keys from the first row, then Submit, three
    // rows down.
    rig.block_on(handle(&rig.cx, message(&now_ts(711), "1 2", Some(&linked))))
        .unwrap();
    assert!(
        last(&rig)
            .body
            .starts_with("🤖 Ticked 1. Cheese, ticked 2. Olives in tab")
    );
    rig.set_screen(&tab, toppings);
    rig.block_on(handle(
        &rig.cx,
        message(&now_ts(712), "anchovies", Some(&linked)),
    ))
    .unwrap();
    rig.set_screen(&tab, toppings);
    rig.block_on(handle(
        &rig.cx,
        message(&now_ts(713), "submit", Some(&linked)),
    ))
    .unwrap();
    let sent: Vec<String> = rig
        .mux
        .sent
        .borrow()
        .iter()
        .filter_map(|s| s.strip_prefix(&format!("{pane} ")).map(str::to_string))
        .collect();
    let joined = sent.join(" | ");
    assert!(
        joined.contains("type 1 | type 2"),
        "both numbers pressed: {joined}"
    );
    assert!(
        joined.contains("key Down | key Down | paste anchovies"),
        "{joined}"
    );
    assert!(
        joined.ends_with("key Down | key Down | key Down | key Enter"),
        "down to Submit, then Enter: {joined}"
    );
    assert!(
        last(&rig)
            .body
            .starts_with("🤖 Submitted your choices in tab")
    );
}

#[test]
fn rename_on_a_thread_header_opens_the_form_there_and_says_the_result_in_the_thread() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    let pid = rig.pid_of(&tab);
    let home = now_ts(800);
    {
        let mut state = rig.cx.state.borrow_mut();
        state.threads.insert(home.clone(), home.clone());
        state.alerts.insert(
            home.clone(),
            Target {
                window: tab.clone(),
                index,
                name: "x".into(),
                thread: None,
            },
        );
        state.homes.insert(
            tab.clone(),
            Home {
                ts: home.clone(),
                pid: pid.clone(),
                ..Home::default()
            },
        );
    }
    rig.slack.answers.borrow_mut().insert(
        "views.open".into(),
        Box::new(|_| Ok(json!({ "ok": true, "view": { "id": "VREN" } }))),
    );
    let t = tabs::tab(&rig.cx, &tab).unwrap();
    let rename = super::buttons::header_buttons(&t)
        .into_iter()
        .find(|b| b.text == "Rename")
        .unwrap();
    let dm = rig.cx.state.borrow().dm();
    let raw = json!({
        "type": "block_actions", "user": { "id": "U1", "team_id": "T1" }, "team": { "id": "T1" }, "trigger_id": "TR7",
        "container": { "channel_id": "D1", "message_ts": home }, "message": { "ts": home, "text": "🤖 Tab" },
        "actions": [{ "value": rename.value.unwrap().to_string() }],
    });
    let p = press_from(&dm, &raw, |_| {}).unwrap();
    rig.block_on(super::presses::handle_press(&rig.cx, p))
        .unwrap();
    let form = rig.calls("views.open")[0]["view"].clone();
    assert_eq!(form["callback_id"], "rename");
    let meta: Value = serde_json::from_str(form["private_metadata"].as_str().unwrap()).unwrap();
    assert_eq!(meta["from"], "thread");
    let mut view = form.clone();
    view["id"] = json!("VREN");
    view["root_view_id"] = json!("VREN");
    view["state"] = json!({ "values": { "name": { "name": { "value": "deploy check" } } } });
    let save = json!({ "type": "view_submission", "user": { "id": "U1", "team_id": "T1" }, "team": { "id": "T1" }, "view": view });
    let submitted = submission_from(&dm, &save, |_| {}).unwrap();
    rig.block_on(super::screens::handle_submission(&rig.cx, &submitted))
        .unwrap();
    assert_eq!(tabs::tab(&rig.cx, &tab).unwrap().own_name, "deploy check");
    assert!(rig.posts().iter().any(|p| {
        p.thread.as_deref() == Some(home.as_str())
            && p.body
                .contains(&format!("Renamed **tab {index}** to **deploy check**"))
    }));
    assert!(
        rig.calls("views.update").is_empty(),
        "no screen under it to draw again"
    );
}

#[test]
fn skill_under_a_finished_turn_opens_a_menu_of_the_tabs_skills_and_run_types_it() {
    let rig = Rig::new();
    let (tab, pane) = rig.claude_tab("work");
    let pid = rig.pid_of(&tab);
    rig.transcript(&tab, "runner", &[json!({
        "type": "attachment",
        "attachment": { "type": "skill_listing", "isInitial": true, "names": ["review-loop"], "content": "- review-loop: Review the branch" },
    })]);
    let thread = now_ts(500);
    {
        let mut state = rig.cx.state.borrow_mut();
        state.threads.insert(thread.clone(), thread.clone());
        state.alerts.insert(
            thread.clone(),
            Target {
                window: tab.clone(),
                index: 1,
                name: "x".into(),
                thread: None,
            },
        );
        state.homes.insert(
            tab.clone(),
            Home {
                ts: thread.clone(),
                pid: pid.clone(),
                ..Home::default()
            },
        );
    }
    rig.slack.answers.borrow_mut().insert(
        "views.open".into(),
        Box::new(|_| Ok(json!({ "ok": true, "view": { "id": "VSKILL" } }))),
    );
    let t = tabs::tab(&rig.cx, &tab).unwrap();
    let skill = super::buttons::turn_buttons(&t)
        .into_iter()
        .find(|b| b.text == "Skill")
        .unwrap();
    let dm = rig.cx.state.borrow().dm();
    let raw = json!({
        "type": "block_actions", "user": { "id": "U1", "team_id": "T1" }, "team": { "id": "T1" }, "trigger_id": "TR9",
        "container": { "channel_id": "D1", "message_ts": "1.2" }, "message": { "ts": "1.2", "thread_ts": thread, "text": "🤖 done" },
        "actions": [{ "value": skill.value.unwrap().to_string() }],
    });
    let p = press_from(&dm, &raw, |_| {}).unwrap();
    rig.block_on(super::presses::handle_press(&rig.cx, p))
        .unwrap();
    let opened = rig.calls("views.open")[0].clone();
    assert_eq!(opened["trigger_id"], "TR9");
    let form = opened["view"].clone();
    assert_eq!(form["callback_id"], "skill");
    let groups: Vec<(String, Vec<String>)> = form["blocks"][0]["element"]["option_groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| {
            (
                g["label"]["text"].as_str().unwrap().to_string(),
                g["options"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|o| o["value"].as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect();
    let review = groups
        .iter()
        .find(|(_, values)| values.iter().any(|v| v == "review-loop"))
        .expect("the session's skill is listed");
    assert!(!review.0.is_empty());
    assert!(
        groups
            .iter()
            .any(|(_, values)| values.iter().any(|v| v == "compact")),
        "Claude Code's own commands are offered"
    );
    assert!(
        !form["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["block_id"] == "typed"),
        "everything fits in the menu"
    );
    let run = |skill: &str, args: &str, user: &str| {
        let mut view = form.clone();
        view["id"] = json!("VSKILL");
        view["root_view_id"] = json!("VSKILL");
        view["state"] = json!({ "values": {
            "skill": { "skill": { "type": "static_select", "selected_option": { "value": skill } } },
            "args": { "args": { "value": args } },
        } });
        json!({ "type": "view_submission", "user": { "id": user, "team_id": "T1" }, "team": { "id": "T1" }, "view": view })
    };
    assert!(
        submission_from(&dm, &run("review-loop", "", "U2"), |_| {}).is_none(),
        "only yours"
    );
    // Not a skill name: nothing typed.
    let bad = submission_from(&dm, &run("rm -rf /", "", "U1"), |_| {});
    if let Some(bad) = bad {
        let _ = rig.block_on(super::screens::handle_submission(&rig.cx, &bad));
    }
    assert!(!rig.posts().iter().any(|p| p.body.contains("Skill:")));
    let good = submission_from(&dm, &run("review-loop", "ABC-12", "U1"), |_| {}).unwrap();
    rig.block_on(super::screens::handle_submission(&rig.cx, &good))
        .unwrap();
    assert!(
        rig.mux
            .sent
            .borrow()
            .iter()
            .any(|s| s.starts_with(&format!("{pane} ")) && s.contains("/review-loop ABC-12")),
        "typed as you would at the keyboard: {:?}",
        rig.mux.sent.borrow()
    );
    assert!(
        rig.posts()
            .iter()
            .any(|p| p.thread.as_deref() == Some(thread.as_str())
                && p.body == "🤖 _Skill:_ `/review-loop ABC-12`"),
        "said in its thread"
    );
    assert!(
        rig.calls("views.update").is_empty(),
        "opened from the thread, so no screen to draw again"
    );
}

#[test]
fn a_skill_menu_lists_switchboard_s_from_its_plugin_as_the_relay_s_settings_say() {
    // The menu's names, as the relay with `settings` draws it for a Claude
    // tab, Switchboard's plugin holding a today.
    let menu = |settings: super::Settings| -> Vec<String> {
        let mut rig = Rig::new();
        std::rc::Rc::get_mut(&mut rig.cx).unwrap().settings = settings;
        let dir = rig.cx.paths.plugin_dir.join("skills/today");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "---\ndescription: The day\n---\n").unwrap();
        let work = rig.root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let (tab, _) = super::mux::Mux::new_tab(&rig.mux, &work, None, None).unwrap();
        rig.mux
            .windows
            .borrow_mut()
            .iter_mut()
            .find(|(w, _)| w.id == tab)
            .unwrap()
            .1[0]
            .info
            .claude = true;
        let t = tabs::tab(&rig.cx, &tab).unwrap();
        let screen = super::screens::skill_screen(&rig.cx, &t, "screen", None, "");
        screen["blocks"][0]["element"]["option_groups"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|g| g["options"].as_array().cloned().unwrap_or_default())
            .map(|o| o["value"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    assert!(
        menu(Box::new(|_| None)).contains(&"today".to_string()),
        "on by default"
    );
    let off = menu(Box::new(|key| {
        (key == crate::setup::way::SETTING).then_some(Value::Bool(false))
    }));
    assert!(!off.contains(&"today".to_string()), "{off:?}");
}

#[test]
fn a_skill_menu_with_more_skills_than_it_holds_cuts_the_rest_and_takes_any_name() {
    let rig = Rig::new();
    let many = rig.root.join("many-skills");
    for i in 0..super::skills::MAX_SKILLS + 5 {
        let dir = many.join(format!(".claude/skills/s{i:03}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "---\ndescription: x\n---\n").unwrap();
    }
    let (tab, _) = super::mux::Mux::new_tab(&rig.mux, &many, None, None).unwrap();
    rig.mux
        .windows
        .borrow_mut()
        .iter_mut()
        .find(|(w, _)| w.id == tab)
        .unwrap()
        .1[0]
        .info
        .claude = true;
    let t = tabs::tab(&rig.cx, &tab).unwrap();
    let big = super::screens::skill_screen(&rig.cx, &t, "screen", None, "");
    let listed: usize = big["blocks"][0]["element"]["option_groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["options"].as_array().unwrap().len())
        .sum();
    assert_eq!(listed, super::skills::MAX_SKILLS);
    let typed = big["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["block_id"] == "typed")
        .expect("a field for any other name");
    let label = typed["label"]["text"].as_str().unwrap();
    assert!(
        label.starts_with("Or another skill (") && label.ends_with(" more aren't in the menu)"),
        "{label}"
    );
    assert_eq!(big["blocks"][0]["optional"], true, "either one will do");
}

#[test]
fn a_tabs_screen_shows_a_waiting_prompts_options_and_is_drawn_again_when_the_tab_changes() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let pid = rig.pid_of(&tab);
    rig.set_screen(&tab, "Allow rm -rf build?\n❯ 1. Yes\n  2. No\n");
    rig.set_state(&tab, "waiting", "1791500200");
    let home = now_ts(400);
    {
        let mut state = rig.cx.state.borrow_mut();
        state.dm.as_mut().unwrap().url = Some("https://example.slack.com/".into());
        state.threads.insert(home.clone(), home.clone());
        state.homes.insert(
            tab.clone(),
            Home {
                ts: home.clone(),
                pid: pid.clone(),
                ..Home::default()
            },
        );
    }
    rig.slack.answers.borrow_mut().insert(
        "views.open".into(),
        Box::new(|_| Ok(json!({ "ok": true, "view": { "id": "VTAB" } }))),
    );
    rig.block_on(super::presses::handle_press(
        &rig.cx,
        super::slack::Press {
            value: json!({ "a": "open", "w": tab, "p": pid }),
            view: Some(super::slack::ViewRef {
                id: "VHOME".into(),
                ..Default::default()
            }),
            trigger_id: Some("TR0".into()),
            ..Default::default()
        },
    ))
    .unwrap();
    let screen = rig.calls("views.open")[0]["view"].clone();
    assert_eq!(
        screen["notify_on_close"], true,
        "Slack says when it is closed"
    );
    let choices = screen["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["type"] == "actions")
        .unwrap()["elements"]
        .clone();
    let names: Vec<&str> = choices
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["text"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["1. Yes", "2. No"]);
    let yes: Value = serde_json::from_str(choices[0]["value"].as_str().unwrap()).unwrap();
    assert_eq!(yes["a"], "screen-opt");
    let dm = rig.cx.state.borrow().dm();
    let press = |value: &Value| {
        let raw = json!({
            "type": "block_actions", "user": { "id": "U1", "team_id": "T1" }, "team": { "id": "T1" }, "trigger_id": "TR1",
            "container": { "type": "view", "view_id": "VTAB" }, "view": { "id": "VTAB", "type": "modal" },
            "actions": [{ "value": value.to_string() }],
        });
        press_from(&dm, &raw, |_| {}).expect("yours, from the screen")
    };
    let last_said = |rig: &Rig| {
        rig.calls("views.update").last().unwrap()["view"]["blocks"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["text"]["text"]
            .as_str()
            .unwrap()
            .to_string()
    };
    // Made for an earlier wait: refused, nothing pressed.
    let mut earlier = yes.clone();
    earlier["s"] = json!("1791500100");
    rig.block_on(super::presses::handle_press(&rig.cx, press(&earlier)))
        .unwrap();
    assert!(last_said(&rig).contains("That choice isn't showing any more"));
    assert!(!rig.screen(&tab).ends_with('1'));
    rig.block_on(super::presses::handle_press(&rig.cx, press(&yes)))
        .unwrap();
    assert!(
        rig.screen(&tab).ends_with("\n1"),
        "only the number, as a reply would be"
    );
    assert!(
        rig.posts()
            .iter()
            .any(|p| p.thread.as_deref() == Some(home.as_str())
                && p.body == "🤖 _From the Home tab:_ chose **1. Yes**"),
        "said in its thread"
    );
    assert!(
        last_said(&rig).starts_with("Chose *1. Yes*. What happens next goes in <https:"),
        "{}",
        last_said(&rig)
    );
    // Nothing changed: not drawn again. Its state changed: drawn again,
    // without the options.
    let drawn = rig.calls("views.update").len();
    let tabs_now = tabs::claude_tabs(&rig.cx);
    rig.block_on(super::screens::refresh_screens(&rig.cx, &tabs_now));
    assert_eq!(rig.calls("views.update").len(), drawn);
    rig.set_state(&tab, "working", "1791500200");
    let tabs_now = tabs::claude_tabs(&rig.cx);
    rig.block_on(super::screens::refresh_screens(&rig.cx, &tabs_now));
    let redrawn = rig.calls("views.update").last().unwrap().clone();
    assert_eq!(redrawn["view_id"], "VTAB");
    assert!(
        redrawn["view"]["blocks"][0]["text"]["text"]
            .as_str()
            .unwrap()
            .contains("🔵 working")
    );
    // Closed in Slack: no longer drawn.
    super::screens::forget_screen(&rig.cx, "VTAB");
    rig.set_state(&tab, "done", "1791500400");
    let drawn = rig.calls("views.update").len();
    let tabs_now = tabs::claude_tabs(&rig.cx);
    rig.block_on(super::screens::refresh_screens(&rig.cx, &tabs_now));
    assert_eq!(rig.calls("views.update").len(), drawn);
}

/// A video attached to a reply under a tab's alert, as Slack hands it over.
fn video_message(rig: &Rig, thread: &str, id: &str, name: &str, bytes: Vec<u8>) -> Message {
    let url = format!("https://files.slack.com/files-pri/T1-{id}/download/{name}");
    let size = bytes.len();
    rig.slack.downloads.borrow_mut().insert(url.clone(), bytes);
    let dm = rig.cx.state.borrow().dm();
    let event = json!({
        "type": "message", "subtype": "file_share", "channel": "D1", "user": "U1", "client_msg_id": "c1",
        "ts": now_ts(41), "thread_ts": thread, "text": "it flickers here",
        "files": [{ "id": id, "name": name, "mimetype": "video/mp4", "size": size, "url_private_download": url }],
    });
    super::slack::message_from(&dm, &event, Some("T1"), |_| {}).unwrap()
}

#[test]
fn a_video_you_attach_is_typed_into_the_tab_by_its_path_and_then_its_stills_in_order() {
    if util::which("ffmpeg").is_none() || util::which("ffprobe").is_none() {
        eprintln!("skipped: needs ffmpeg");
        return;
    }
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let thread = alert(&rig, &tab, rig.index_of(&tab), "work");
    // A real three-second video, made by ffmpeg.
    let recording = rig.root.join("recording.mp4");
    let made = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc=duration=3:size=1920x1080:rate=30")
        .args(["-pix_fmt", "yuv420p"])
        .arg(&recording)
        .status()
        .unwrap();
    assert!(made.success());
    let m = video_message(
        &rig,
        &thread,
        "F0REC",
        "flicker.mp4",
        std::fs::read(&recording).unwrap(),
    );
    rig.block_on(handle(&rig.cx, m)).unwrap();
    let saved = rig.cx.paths.files_dir().join("F0REC-flicker.mp4");
    let stills: Vec<std::path::PathBuf> = (1..=super::files::FRAMES)
        .map(|i| std::path::PathBuf::from(format!("{}-frame-{i:02}.jpg", saved.display())))
        .collect();
    for still in &stills {
        assert!(still.exists(), "{} was taken", still.display());
    }
    assert_eq!(
        image::image_dimensions(&stills[0]).unwrap().0,
        1280,
        "scaled down to be read, not kept full size"
    );
    let expected = format!(
        "it flickers here [Video: {}] Stills from it, in order: [Image: {}] [Image: {}]",
        saved.display(),
        stills[0].display(),
        stills[1].display()
    );
    assert!(rig.screen(&tab).contains(&expected), "{}", rig.screen(&tab));
    assert!(!rig.posts().iter().any(|p| p.body.contains("take stills")));
}

#[test]
fn a_file_that_only_claims_to_be_a_video_gets_no_stills_and_you_are_told() {
    let rig = Rig::new();
    let (tab, _) = rig.claude_tab("work");
    let thread = alert(&rig, &tab, rig.index_of(&tab), "work");
    // An MP4's first bytes, so it is taken as one, and nothing a player
    // could read after them.
    let mut bytes = b"\0\0\0\x18ftypmp42".to_vec();
    bytes.extend_from_slice(b"not a video");
    let m = video_message(&rig, &thread, "F0BAD", "bad.mp4", bytes);
    rig.block_on(handle(&rig.cx, m)).unwrap();
    let saved = rig.cx.paths.files_dir().join("F0BAD-bad.mp4");
    assert!(
        rig.screen(&tab)
            .contains(&format!("[Video: {}]", saved.display()))
    );
    assert!(rig.posts().iter().any(|p| {
        p.body
            .contains("couldn't take stills from bad.mp4, so the tab gets only its path")
    }));
}

#[test]
fn opening_a_tabs_screen_starts_its_thread_and_links_it() {
    let rig = Rig::new();
    rig.cx.state.borrow_mut().dm.as_mut().unwrap().url = Some("https://example.slack.com/".into());
    let (tab, _) = rig.claude_tab("work");
    let pid = rig.pid_of(&tab);
    let open = || {
        rig.block_on(super::presses::handle_press(
            &rig.cx,
            super::slack::Press {
                value: json!({ "a": "open", "w": tab, "p": pid }),
                view: Some(super::slack::ViewRef {
                    id: "VHOME".into(),
                    ..Default::default()
                }),
                trigger_id: Some("TR0".into()),
                ..Default::default()
            },
        ))
        .unwrap();
    };
    let link = |screen: &Value| {
        screen["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["type"] == "actions")
            .unwrap()["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["text"]["text"] == "Go to thread")
            .map(|b| b["url"].as_str().unwrap().to_string())
    };
    open();
    let posts = rig.posts();
    assert_eq!(posts.len(), 1, "the thread's header");
    assert!(posts[0].thread.is_none());
    let ts = posts[0].ts.clone();
    let redrawn = rig.calls("views.update");
    assert_eq!(redrawn.len(), 1);
    assert_eq!(redrawn[0]["view_id"], "V-opened");
    let url = link(&redrawn[0]["view"]).expect("Go to thread");
    assert!(url.contains(&format!("thread_ts={ts}")), "{url}");
    open();
    assert_eq!(rig.posts().len(), 1, "no second header");
    assert_eq!(rig.calls("views.update").len(), 1, "no redraw needed");
    assert_eq!(link(&rig.calls("views.open")[1]["view"]), Some(url));
}

// ── What reaches you (src/relay/sending.rs) ─────────────────────────────────

/// A rig whose settings are `pairs`, changed later through what it gives.
fn rig_with(
    pairs: &[(&str, Value)],
) -> (
    Rig,
    std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, Value>>>,
) {
    let mut rig = Rig::new();
    let settings = std::rc::Rc::new(std::cell::RefCell::new(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect::<std::collections::HashMap<_, _>>(),
    ));
    let read = settings.clone();
    std::rc::Rc::get_mut(&mut rig.cx).unwrap().settings =
        Box::new(move |k| read.borrow().get(k).cloned());
    (rig, settings)
}

fn alert_now(rig: &Rig) {
    let tabs = tabs::claude_tabs(&rig.cx);
    rig.block_on(alerts::alert_tabs(&rig.cx, &tabs, &[]));
}

#[test]
fn a_finished_turn_turned_off_posts_nothing_even_once_turned_back_on() {
    let (rig, settings) = rig_with(&[("relay_send_finished", json!("off"))]);
    let (tab, _) = rig.claude_tab("work");
    rig.set_state(&tab, "done", "100");
    alert_now(&rig);
    assert!(rig.posts().is_empty());
    settings
        .borrow_mut()
        .insert("relay_send_finished".into(), json!("thread"));
    alert_now(&rig);
    assert!(rig.posts().is_empty(), "the turn counted as reported");
    let sent = std::fs::read_dir(rig.cx.paths.state_dir.join("sent"));
    assert!(sent.is_err(), "nothing recorded as sent");
}

#[test]
fn a_prompt_set_to_its_thread_posts_no_line_in_the_main_dm() {
    let (rig, _) = rig_with(&[("relay_send_prompts", json!("thread"))]);
    let (tab, _) = rig.claude_tab("work");
    rig.set_state(&tab, "waiting", "100");
    alert_now(&rig);
    let posts = rig.posts();
    assert_eq!(posts.len(), 2, "the thread and the alert in it");
    assert!(posts[1].body.contains("is **waiting on you**"));
    assert!(posts.iter().all(|p| !p.broadcast));
    // Recorded as sent, not notified.
    let day = util::ymd(&util::local_now());
    let text =
        std::fs::read_to_string(super::sending::sent_file(&rig.cx.paths.state_dir, &day)).unwrap();
    let sent = super::sending::read_sent(&text);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].kind, super::sending::Kind::Prompts);
    assert!(!sent[0].notified);
}

#[test]
fn a_finished_turn_set_to_notify_gets_a_line_in_the_main_dm() {
    let (rig, _) = rig_with(&[("relay_send_finished", json!("notify"))]);
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    rig.set_state(&tab, "done", "100");
    alert_now(&rig);
    let posts = rig.posts();
    assert_eq!(posts.len(), 3);
    assert!(posts[2].broadcast);
    assert_eq!(
        posts[2].body,
        format!("🤖 **Tab {index}, work** has finished.")
    );
}

#[test]
fn an_autopilot_tab_under_landings_posts_its_waits_but_not_its_turns() {
    let (rig, _) = rig_with(&[("relay_send_autopilot", json!("landings"))]);
    let (pilot, _) = rig.claude_tab("pilot");
    rig.set_autopilot(&pilot);
    let (mine, _) = rig.claude_tab("mine");
    rig.set_state(&pilot, "done", "100");
    rig.set_state(&mine, "done", "100");
    alert_now(&rig);
    let finished: Vec<_> = rig
        .posts()
        .into_iter()
        .filter(|p| p.body.contains("has finished"))
        .collect();
    assert_eq!(finished.len(), 1);
    assert!(finished[0].body.contains("mine"));
    rig.set_state(&pilot, "waiting", "200");
    alert_now(&rig);
    assert!(
        rig.posts()
            .iter()
            .any(|p| p.broadcast && p.body.contains("pilot** is **waiting on you**."))
    );
}

#[test]
fn an_answer_to_your_message_reaches_you_with_finished_turns_off() {
    let (rig, _) = rig_with(&[("relay_send_finished", json!("off"))]);
    let (tab, _) = rig.claude_tab("work");
    let index = rig.index_of(&tab);
    let transcript = rig.transcript(&tab, "asked", &[
        json!({ "type": "user", "message": { "content": "deploy it" } }),
        json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Deployed." }] } }),
    ]);
    rig.events(&[
        json!({ "event": "done", "time": util::now_secs(), "transcript_path": transcript }),
    ]);
    rig.cx
        .state
        .borrow_mut()
        .following
        .insert(tab.clone(), follow(None, index, "deploy it"));
    ticks(&rig);
    let posts = rig.posts();
    assert_eq!(posts.len(), 1);
    assert!(posts[0].body.contains("Deployed."));
}

#[test]
fn a_notice_turned_off_is_refused_and_its_sender_told() {
    let (rig, _) = rig_with(&[("relay_send_notices", json!("off"))]);
    let dir = rig.cx.paths.outbox_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let id = format!("{}-1-abc", util::now_ms());
    std::fs::write(
        dir.join(format!("{id}.json")),
        json!({ "from": "oncall", "text": "disk full" }).to_string(),
    )
    .unwrap();
    rig.block_on(super::outbox::deliver_outbox(&rig.cx));
    assert!(rig.posts().is_empty());
    let why = std::fs::read_to_string(dir.join(format!("{id}.failed"))).unwrap();
    assert!(why.contains("relay_send_notices"), "{why}");
}

#[test]
fn a_full_context_is_warned_of_with_finished_turns_off() {
    let (rig, _) = rig_with(&[("relay_send_finished", json!("off"))]);
    let (tab, _) = rig.claude_tab("work");
    rig.transcript(
        &tab,
        "full",
        &[json!({ "type": "user", "message": { "content": "go" } })],
    );
    let context = rig.cx.paths.context_dirs[0].join("full");
    std::fs::create_dir_all(context.parent().unwrap()).unwrap();
    std::fs::write(&context, "93\tOpus").unwrap();
    rig.set_state(&tab, "done", "100");
    alert_now(&rig);
    let posts = rig.posts();
    assert!(posts.iter().all(|p| !p.body.contains("has finished")));
    assert!(
        posts
            .iter()
            .any(|p| p.body.contains("has used **93%** of its context")),
        "{posts:?}"
    );
    let warnings = |rig: &Rig| {
        rig.posts()
            .iter()
            .filter(|p| p.body.contains("of its context. Compact"))
            .count()
    };
    // Compacted below it, then full again: warned again.
    std::fs::write(&context, "40\tOpus").unwrap();
    rig.set_state(&tab, "done", "200");
    alert_now(&rig);
    std::fs::write(&context, "91\tOpus").unwrap();
    rig.set_state(&tab, "done", "300");
    alert_now(&rig);
    assert_eq!(warnings(&rig), 2);
}

#[test]
fn a_full_context_not_warned_of_is_no_backlog_once_turned_on() {
    let (rig, settings) = rig_with(&[
        ("relay_send_finished", json!("off")),
        ("relay_send_context", json!("off")),
    ]);
    let (tab, _) = rig.claude_tab("work");
    rig.transcript(
        &tab,
        "full",
        &[json!({ "type": "user", "message": { "content": "go" } })],
    );
    let context = rig.cx.paths.context_dirs[0].join("full");
    std::fs::create_dir_all(context.parent().unwrap()).unwrap();
    std::fs::write(&context, "93\tOpus").unwrap();
    rig.set_state(&tab, "done", "100");
    alert_now(&rig);
    assert!(rig.posts().is_empty());
    settings
        .borrow_mut()
        .insert("relay_send_context".into(), json!("thread"));
    rig.set_state(&tab, "done", "200");
    alert_now(&rig);
    assert!(rig.posts().is_empty(), "{:?}", rig.posts());
}

#[test]
fn a_held_line_is_sent_as_its_kind_says_when_it_goes_out_and_counted_then() {
    let (rig, settings) = rig_with(&[]);
    rig.at_computer();
    let (tab, _) = rig.claude_tab("work");
    rig.set_state(&tab, "waiting", "100");
    alert_now(&rig);
    let day = util::ymd(&util::local_now());
    let file = super::sending::sent_file(&rig.cx.paths.state_dir, &day);
    let notified = || {
        super::sending::read_sent(&std::fs::read_to_string(&file).unwrap_or_default())
            .iter()
            .filter(|s| s.notified)
            .count()
    };
    // Held while you're at the computer: sent, not yet notified.
    assert_eq!(rig.cx.presence.held.borrow().len(), 1);
    assert_eq!(notified(), 0);
    // Turned to the thread alone meanwhile: the held line goes nowhere.
    settings
        .borrow_mut()
        .insert("relay_send_prompts".into(), json!("thread"));
    rig.set_reading(Some(super::presence::AWAY_AFTER_MS), Some(false));
    rig.block_on(super::presence::flush(&rig.cx));
    assert!(rig.posts().iter().all(|p| !p.broadcast));
    assert_eq!(notified(), 0);
    // Left to notify, it is counted once posted.
    settings
        .borrow_mut()
        .insert("relay_send_prompts".into(), json!("notify"));
    rig.at_computer();
    rig.set_state(&tab, "waiting", "200");
    alert_now(&rig);
    rig.set_reading(Some(super::presence::AWAY_AFTER_MS), Some(false));
    rig.block_on(super::presence::flush(&rig.cx));
    assert!(rig.posts().iter().any(|p| p.broadcast));
    assert_eq!(notified(), 1);
}
