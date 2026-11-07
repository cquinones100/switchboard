//! The relay's behaviour end to end, ported from test/slack-relay.test.js:
//! messages handled, tabs typed into and followed, alerts, the Home tab,
//! screens, buttons and forms, notices, the watchdog. Each runs a relay
//! with stand-ins for Slack, the tabs and Haiku (src/relay/testing.rs), so
//! none reaches Slack, a server, or your ~/.claude.

use std::time::Duration;

use serde_json::{Value, json};

use super::commands::handle;
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
    let view = super::dashboard::dashboard_view(&rig.cx, &rows, None);
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
    let stopped = super::dashboard::dashboard_view(&rig.cx, &rows, Some(util::now_ms()));
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
        true,
    ))
    .unwrap();
    rig.block_on(super::milestones::ping(
        &rig.cx,
        &t,
        "pushed `feature`",
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
        "no Stop for an idle tab, no thread it hasn't got"
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
