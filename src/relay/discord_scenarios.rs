//! The relay through a Discord bot's DM, end to end: what you send and
//! press reaches the tabs only from you, in the DM; a tab's messages are
//! replies to its header, silent or not as the settings say; the pinned
//! list opens a tab's screen; forms open as modals. Each runs a relay with
//! stand-ins for Discord, the tabs and Haiku (src/relay/testing.rs), so
//! none reaches Discord, a server, or your ~/.claude.

use serde_json::{Value, json};

use super::dashboard::{Refresh, refresh_dashboard};
use super::discord::{SUPPRESS_NOTIFICATIONS, snowflake_at};
use super::post::{PostOpts, post};
use super::testing::{DISCORD_BOT, DISCORD_DM, DISCORD_OWNER, RestCall, Rig};
use super::{alerts, tabs};
use crate::util;

/// A message you typed in the DM, replying to `reply_to` if given.
fn typed(rig: &Rig, text: &str, reply_to: Option<&str>) -> Value {
    let mut m = json!({
        "id": rig.discord.next_id(),
        "type": if reply_to.is_some() { 19 } else { 0 },
        "channel_id": DISCORD_DM,
        "author": { "id": DISCORD_OWNER },
        "content": text,
    });
    if let Some(r) = reply_to {
        m["message_reference"] = json!({ "message_id": r, "channel_id": DISCORD_DM });
    }
    m
}

/// A press of the component `custom_id` on the relay's message `on`.
fn press(on: &str, custom_id: &str, values: &[&str]) -> Value {
    json!({
        "id": format!("I{}", util::now_ms()),
        "token": "tok",
        "type": 3,
        "channel_id": DISCORD_DM,
        "user": { "id": DISCORD_OWNER },
        "data": { "custom_id": custom_id, "component_type": if values.is_empty() { 2 } else { 3 }, "values": values },
        "message": { "id": on, "content": "", "author": { "id": DISCORD_BOT } },
    })
}

/// A form sent, its fields' values.
fn submitted(custom_id: &str, fields: &[(&str, &str)]) -> Value {
    json!({
        "id": format!("S{}", util::now_ms()),
        "token": "tok",
        "type": 5,
        "channel_id": DISCORD_DM,
        "user": { "id": DISCORD_OWNER },
        "data": {
            "custom_id": custom_id,
            "components": fields.iter().map(|(id, v)| json!({ "type": 18, "component": { "type": 4, "custom_id": id, "value": v } })).collect::<Vec<_>>(),
        },
    })
}

/// The `custom_id` of the button labelled `label` in a posted message.
fn button(message: &Value, label: &str) -> String {
    message["components"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|row| row["components"].as_array().cloned().unwrap_or_default())
        .find(|b| b["label"] == label)
        .and_then(|b| b["custom_id"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("no {label} button in {message}"))
}

/// The last tab's screen posted, which its title heads.
fn last_screen(rig: &Rig) -> Value {
    rig.discord
        .posted()
        .into_iter()
        .rev()
        .find(|m| m["content"].as_str().is_some_and(|c| c.starts_with("### ")))
        .expect("a screen posted")
}

/// Every answer to an interaction, in order.
fn answers(rig: &Rig) -> Vec<RestCall> {
    rig.discord.calls_to("POST", "/interactions/")
}

/// The tab's header message, made now.
fn header(rig: &Rig, tab: &str) -> String {
    let t = tabs::tab(&rig.cx, tab).unwrap();
    rig.block_on(super::homes::home_of(&rig.cx, &t))
        .unwrap()
        .unwrap()
}

fn flags(message: &Value) -> u64 {
    message["flags"].as_u64().unwrap_or(0)
}

fn log(rig: &Rig) -> String {
    util::read_text(rig.cx.paths.log_file()).unwrap_or_default()
}

#[test]
fn a_reply_to_a_tabs_header_or_to_a_reply_under_it_types_into_that_tab() {
    let rig = Rig::discord();
    let (tab, pane) = rig.claude_tab("work");
    let head = header(&rig, &tab);
    rig.dispatch("MESSAGE_CREATE", typed(&rig, "go on", Some(&head)));
    assert!(
        rig.mux
            .sent
            .borrow()
            .contains(&format!("{pane} paste go on")),
        "{:?}",
        rig.mux.sent.borrow()
    );
    // The relay's own reply in the thread is a reply to the header, and
    // replying to it is replying in the same thread.
    let reply = rig
        .block_on(post(&rig.cx, "🤖 done", Some(&head), PostOpts::default()))
        .unwrap();
    let posted = rig.discord.posted();
    assert_eq!(
        posted.last().unwrap()["message_reference"]["message_id"],
        head.as_str()
    );
    rig.mux.sent.borrow_mut().clear();
    rig.dispatch("MESSAGE_CREATE", typed(&rig, "and this", Some(&reply)));
    assert!(
        rig.mux
            .sent
            .borrow()
            .contains(&format!("{pane} paste and this")),
        "{:?}",
        rig.mux.sent.borrow()
    );
}

#[test]
fn messages_from_anyone_else_a_bot_a_webhook_or_a_server_are_ignored_and_logged() {
    let rig = Rig::discord();
    let (tab, _) = rig.claude_tab("work");
    let head = header(&rig, &tab);
    let posts = rig.discord.posted().len();
    let mut others = Vec::new();
    for (pointer, value) in [
        ("/author/id", json!("1999999999999999999")),
        ("/channel_id", json!("1888888888888888888")),
        ("/guild_id", json!("1777777777777777777")),
        ("/webhook_id", json!("1666666666666666666")),
    ] {
        let mut m = typed(&rig, "status", Some(&head));
        match m.pointer_mut(pointer) {
            Some(v) => *v = value,
            None => m[pointer.trim_start_matches('/')] = value,
        }
        others.push(m);
    }
    let mut bot = typed(&rig, "status", None);
    bot["author"]["bot"] = json!(true);
    others.push(bot);
    for m in others {
        rig.dispatch("MESSAGE_CREATE", m);
    }
    assert!(rig.mux.sent.borrow().is_empty(), "nothing typed");
    assert_eq!(rig.discord.posted().len(), posts, "nothing answered");
    assert_eq!(
        log(&rig).matches("ignored a Discord message").count(),
        4,
        "each logged but the bot's"
    );
}

#[test]
fn a_tabs_messages_are_silent_on_thread_only_and_notify_on_notify_me() {
    let rig = Rig::discord();
    let (tab, _) = rig.claude_tab("work");
    let head = header(&rig, &tab);
    assert_eq!(
        flags(&rig.discord.posted()[0]) & SUPPRESS_NOTIFICATIONS,
        0,
        "a header in the main DM notifies"
    );
    rig.block_on(post(&rig.cx, "quiet", Some(&head), PostOpts::default()))
        .unwrap();
    rig.block_on(post(&rig.cx, "loud", Some(&head), PostOpts::mention(true)))
        .unwrap();
    let posted = rig.discord.posted();
    let quiet = &posted[posted.len() - 2];
    let loud = &posted[posted.len() - 1];
    assert_ne!(flags(quiet) & SUPPRESS_NOTIFICATIONS, 0, "{quiet}");
    assert_eq!(flags(loud) & SUPPRESS_NOTIFICATIONS, 0, "{loud}");
}

/// A Discord rig whose settings are `pairs`.
fn rig_with(pairs: &[(&str, Value)]) -> Rig {
    let mut rig = Rig::discord();
    let settings: std::collections::HashMap<String, Value> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    std::rc::Rc::get_mut(&mut rig.cx).unwrap().settings =
        Box::new(move |k| settings.get(k).cloned());
    rig
}

#[test]
fn each_send_choice_holds_notify_posts_a_normal_reply_thread_a_silent_one_off_nothing() {
    let waiting = |choice: &str| {
        let rig = rig_with(&[("relay_send_prompts", json!(choice))]);
        let (tab, _) = rig.claude_tab("work");
        rig.set_state(&tab, "waiting", "100");
        let tabs = tabs::claude_tabs(&rig.cx);
        rig.block_on(alerts::alert_tabs(&rig.cx, &tabs, &[]));
        rig.discord.posted()
    };
    let thread = waiting("thread");
    assert!(thread.len() >= 2);
    assert!(
        thread[1..]
            .iter()
            .all(|m| flags(m) & SUPPRESS_NOTIFICATIONS != 0),
        "{thread:?}"
    );
    let notify = waiting("notify");
    assert!(
        notify[1..]
            .iter()
            .any(|m| flags(m) & SUPPRESS_NOTIFICATIONS == 0),
        "{notify:?}"
    );
    let finished = |choice: &str| {
        let rig = rig_with(&[("relay_send_finished", json!(choice))]);
        let (tab, _) = rig.claude_tab("work");
        rig.set_state(&tab, "done", "100");
        let tabs = tabs::claude_tabs(&rig.cx);
        rig.block_on(alerts::alert_tabs(&rig.cx, &tabs, &[]));
        rig.discord.posted()
    };
    assert!(finished("off").is_empty(), "Don't send posts nothing");
}

#[test]
fn a_press_from_the_owner_opens_its_form_and_the_form_renames_the_tab() {
    let rig = Rig::discord();
    let (tab, _) = rig.claude_tab("work");
    let head = header(&rig, &tab);
    let rename = button(&rig.discord.posted()[0], "Rename");
    // Someone else's press is never answered.
    let mut theirs = press(&head, &rename, &[]);
    theirs["user"]["id"] = json!("1999999999999999999");
    rig.dispatch("INTERACTION_CREATE", theirs);
    assert!(answers(&rig).is_empty());
    assert!(log(&rig).contains("ignored a Discord press"));
    rig.dispatch("INTERACTION_CREATE", press(&head, &rename, &[]));
    let opened = answers(&rig);
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert_eq!(opened[0].body["type"], 9, "a modal");
    let modal = &opened[0].body["data"];
    let custom = modal["custom_id"].as_str().unwrap().to_string();
    assert_eq!(modal["components"][0]["component"]["custom_id"], "name");
    // Someone else's form is never taken.
    let mut form = submitted(&custom, &[("name", "Fix the login")]);
    form["user"]["id"] = json!("1999999999999999999");
    rig.dispatch("INTERACTION_CREATE", form);
    assert!(rig.mux.windows.borrow()[0].0.own_name.is_empty());
    rig.dispatch(
        "INTERACTION_CREATE",
        submitted(&custom, &[("name", "Fix the login")]),
    );
    assert_eq!(rig.mux.windows.borrow()[0].0.own_name, "Fix the login");
    let acks = answers(&rig);
    assert_eq!(
        acks.last().unwrap().body["type"],
        6,
        "the form acknowledged"
    );
}

#[test]
fn the_pinned_list_is_posted_pinned_then_edited_and_its_menu_opens_a_tabs_screen() {
    let rig = Rig::discord();
    let (tab, _) = rig.claude_tab("work");
    rig.block_on(refresh_dashboard(&rig.cx, Refresh::opened_quick()))
        .unwrap();
    let list = rig.discord.posted()[0].clone();
    let id = list["id"].as_str().unwrap().to_string();
    assert_eq!(
        rig.discord
            .calls_to("PUT", &format!("/channels/{DISCORD_DM}/pins/{id}"))
            .len(),
        1
    );
    assert!(list["content"].as_str().unwrap().contains("work"));
    // A tab opening edits it in place.
    rig.claude_tab("other");
    rig.block_on(refresh_dashboard(&rig.cx, Refresh::opened_quick()))
        .unwrap();
    let edits = rig
        .discord
        .calls_to("PATCH", &format!("/channels/{DISCORD_DM}/messages/{id}"));
    assert_eq!(edits.len(), 1);
    assert!(edits[0].body["content"].as_str().unwrap().contains("other"));
    assert_eq!(rig.discord.posted().len(), 1, "nothing posted again");
    // Its menu opens the tab's screen as a message with its buttons.
    let menu = &edits[0].body["components"][0]["components"][0];
    assert_eq!(menu["custom_id"], "home");
    let option = menu["options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["label"].as_str().unwrap().contains("work"))
        .unwrap()["value"]
        .as_str()
        .unwrap()
        .to_string();
    rig.dispatch("INTERACTION_CREATE", press(&id, "home", &[&option]));
    let screen = last_screen(&rig);
    assert!(screen["content"].as_str().unwrap().contains("work"));
    for label in ["Message", "Skill", "Progress", "Rename", "Close"] {
        button(&screen, label);
    }
    // A tab that is working can be stopped from it.
    rig.set_state(&tab, "working", "100");
    rig.dispatch("INTERACTION_CREATE", press(&id, "home", &[&option]));
    button(&last_screen(&rig), "Stop");
}

#[test]
fn a_screens_message_opens_a_modal_and_sends_into_the_tab() {
    let rig = Rig::discord();
    let (tab, pane) = rig.claude_tab("work");
    rig.block_on(refresh_dashboard(&rig.cx, Refresh::opened_quick()))
        .unwrap();
    let list = rig.discord.posted()[0].clone();
    let id = list["id"].as_str().unwrap().to_string();
    let option = list["components"][0]["components"][0]["options"][0]["value"]
        .as_str()
        .unwrap()
        .to_string();
    rig.dispatch("INTERACTION_CREATE", press(&id, "home", &[&option]));
    let screen = last_screen(&rig);
    let screen_id = screen["id"].as_str().unwrap().to_string();
    rig.dispatch(
        "INTERACTION_CREATE",
        press(&screen_id, &button(&screen, "Message"), &[]),
    );
    let modal = answers(&rig).last().unwrap().body.clone();
    assert_eq!(modal["type"], 9);
    let custom = modal["data"]["custom_id"].as_str().unwrap().to_string();
    rig.dispatch(
        "INTERACTION_CREATE",
        submitted(&custom, &[("text", "please continue")]),
    );
    assert!(
        rig.mux
            .sent
            .borrow()
            .contains(&format!("{pane} paste please continue")),
        "{:?}",
        rig.mux.sent.borrow()
    );
    // The screen is drawn again in place, saying it was sent.
    let redrawn = rig.discord.calls_to(
        "PATCH",
        &format!("/channels/{DISCORD_DM}/messages/{screen_id}"),
    );
    assert!(
        redrawn.last().unwrap().body["content"]
            .as_str()
            .unwrap()
            .contains("Sent."),
        "{redrawn:?}"
    );
    let _ = tab;
}

#[test]
fn a_button_slack_would_confirm_asks_first_in_a_message_only_you_see() {
    let rig = Rig::discord();
    let ts = rig
        .block_on(super::run::say_listening(
            &rig.cx,
            &super::run::welcome_text(""),
            "",
        ))
        .unwrap();
    let welcome = rig.discord.posted()[0].clone();
    let stop = button(&welcome, "Stop relay");
    assert!(stop.starts_with("c:"));
    rig.dispatch("INTERACTION_CREATE", press(&ts, &stop, &[]));
    let asked = answers(&rig).last().unwrap().body.clone();
    assert_eq!(asked["type"], 4);
    assert_eq!(asked["data"]["flags"], json!(super::discord::EPHEMERAL));
    assert!(
        asked["data"]["content"]
            .as_str()
            .unwrap()
            .contains("Stop the relay?")
    );
    assert!(!rig.exited.get(), "nothing stopped yet");
}

#[test]
fn close_confirmed_on_a_screen_closes_its_tab_and_says_so_on_the_screen() {
    let rig = Rig::discord();
    let (tab, _) = rig.claude_tab("work");
    rig.block_on(refresh_dashboard(&rig.cx, Refresh::opened_quick()))
        .unwrap();
    let list = rig.discord.posted()[0].clone();
    let id = list["id"].as_str().unwrap().to_string();
    let option = list["components"][0]["components"][0]["options"][0]["value"]
        .as_str()
        .unwrap()
        .to_string();
    rig.dispatch("INTERACTION_CREATE", press(&id, "home", &[&option]));
    let screen = last_screen(&rig);
    let screen_id = screen["id"].as_str().unwrap().to_string();
    rig.dispatch(
        "INTERACTION_CREATE",
        press(&screen_id, &button(&screen, "Close"), &[]),
    );
    let asked = answers(&rig).last().unwrap().body.clone();
    let yes = asked["data"]["components"][0]["components"][0]["custom_id"]
        .as_str()
        .unwrap()
        .to_string();
    rig.dispatch(
        "INTERACTION_CREATE",
        press("1999000000000000000", &yes, &[]),
    );
    assert_eq!(*rig.mux.killed.borrow(), [tab]);
    let redrawn = rig.discord.calls_to(
        "PATCH",
        &format!("/channels/{DISCORD_DM}/messages/{screen_id}"),
    );
    assert!(
        redrawn.last().unwrap().body["content"]
            .as_str()
            .unwrap()
            .contains("Closed"),
        "{redrawn:?}"
    );
}

#[test]
fn notify_posts_in_the_discord_dm() {
    let rig = Rig::discord();
    let paths = rig.cx.paths.clone();
    util::write_private(
        paths.lock_file(),
        format!("{} slack-relay\n", std::process::id()),
        false,
    )
    .unwrap();
    let notifier = std::thread::spawn(move || {
        super::outbox::notify(
            &paths,
            "Deploy finished",
            None,
            std::time::Duration::from_secs(10),
        )
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while rig.discord.posted().is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
        rig.block_on(super::outbox::deliver_outbox(&rig.cx));
    }
    assert!(notifier.join().unwrap().is_ok());
    let posted = rig.discord.posted();
    assert!(
        posted[0]["content"]
            .as_str()
            .unwrap()
            .contains("Deploy finished")
    );
}

#[test]
fn every_call_naming_another_channel_is_refused_before_it_is_sent() {
    let rig = Rig::discord();
    let chat = rig.discord_chat.clone().unwrap();
    let refused = rig
        .block_on(chat.0.discord.api(
            "POST",
            "/channels/1999999999999999999/messages",
            Some(json!({ "content": "x" })),
        ))
        .unwrap_err();
    assert!(refused.to_string().contains("only uses its DM"));
    assert!(rig.discord.calls.borrow().is_empty(), "nothing sent");
}

#[test]
fn catching_up_reads_your_messages_since_the_cursor_and_in_watched_threads() {
    let rig = Rig::discord();
    let (tab, _) = rig.claude_tab("work");
    let head = header(&rig, &tab);
    let before = snowflake_at(util::now_ms() as u64 - 60_000);
    rig.cx.state.borrow_mut().cursor = Some(format!("{:.6}", util::now_ms() as f64 / 1000.0 - 1.0));
    let old = json!({ "id": before, "type": 0, "channel_id": DISCORD_DM, "author": { "id": DISCORD_OWNER }, "content": "old" });
    let top = typed(&rig, "status", None);
    let reply = typed(&rig, "go on", Some(&head));
    *rig.discord.history.borrow_mut() = vec![reply, old, top];
    let found = rig.block_on(super::read::catch_up(&rig.cx)).unwrap();
    let texts: Vec<(&str, Option<&str>)> = found
        .iter()
        .map(|m| (m.text.as_str(), m.thread_ts.as_deref()))
        .collect();
    assert_eq!(texts, [("status", None), ("go on", Some(head.as_str()))]);
}

#[test]
fn skill_starts_as_a_menu_of_skills_whose_choice_opens_its_modal() {
    let rig = Rig::discord();
    let (_, pane) = rig.claude_tab("work");
    rig.block_on(refresh_dashboard(&rig.cx, Refresh::opened_quick()))
        .unwrap();
    let list = rig.discord.posted()[0].clone();
    let id = list["id"].as_str().unwrap().to_string();
    let option = list["components"][0]["components"][0]["options"][0]["value"]
        .as_str()
        .unwrap()
        .to_string();
    rig.dispatch("INTERACTION_CREATE", press(&id, "home", &[&option]));
    let screen = last_screen(&rig);
    let screen_id = screen["id"].as_str().unwrap().to_string();
    rig.dispatch(
        "INTERACTION_CREATE",
        press(&screen_id, &button(&screen, "Skill"), &[]),
    );
    let picker = last_screen(&rig);
    let picker_id = picker["id"].as_str().unwrap().to_string();
    let menu = &picker["components"][0]["components"][0];
    assert_eq!(menu["custom_id"], "fs:skill", "{picker}");
    let skill = menu["options"][0]["value"].as_str().unwrap().to_string();
    rig.dispatch(
        "INTERACTION_CREATE",
        press(&picker_id, "fs:skill", &[&skill]),
    );
    let modal = answers(&rig).last().unwrap().body.clone();
    assert_eq!(modal["type"], 9, "{modal}");
    let fields: Vec<&str> = modal["data"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["component"]["custom_id"].as_str().unwrap())
        .collect();
    assert!(
        fields.contains(&"args") && !fields.contains(&"skill"),
        "{fields:?}"
    );
    let custom = modal["data"]["custom_id"].as_str().unwrap().to_string();
    rig.dispatch("INTERACTION_CREATE", submitted(&custom, &[("args", "now")]));
    assert!(
        rig.mux
            .sent
            .borrow()
            .contains(&format!("{pane} paste /{skill} now")),
        "{:?}",
        rig.mux.sent.borrow()
    );
    assert_eq!(
        rig.discord
            .calls_to(
                "DELETE",
                &format!("/channels/{DISCORD_DM}/messages/{picker_id}")
            )
            .len(),
        1,
        "the menu goes once the skill is sent"
    );
}

#[test]
fn discords_state_is_kept_apart_from_slacks() {
    let slack = Rig::new();
    let discord = Rig::discord();
    assert!(
        slack
            .cx
            .paths
            .state_file()
            .ends_with("slack-relay/state.json")
    );
    assert!(
        discord
            .cx
            .paths
            .state_file()
            .ends_with("slack-relay/discord-state.json")
    );
    assert!(
        discord
            .cx
            .paths
            .config_file()
            .ends_with(".config/switchboard/discord-relay.json")
    );
}

#[test]
fn catching_up_reaches_what_you_sent_last_past_a_long_thread_of_the_relays() {
    let rig = Rig::discord();
    let (tab, _) = rig.claude_tab("work");
    let head = header(&rig, &tab);
    rig.cx.state.borrow_mut().cursor = Some(head.clone());
    let mut history: Vec<Value> = (0..1_200)
        .map(|_| {
            json!({ "id": rig.discord.next_id(), "type": 19, "channel_id": DISCORD_DM,
                "author": { "id": DISCORD_BOT, "bot": true }, "content": "🤖 a turn",
                "message_reference": { "message_id": head } })
        })
        .collect();
    history.push(typed(&rig, "go on", Some(&head)));
    *rig.discord.history.borrow_mut() = history;
    let found = rig.block_on(super::read::catch_up(&rig.cx)).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].text, "go on");
    assert_eq!(found[0].thread_ts.as_deref(), Some(head.as_str()));
}

#[test]
fn deleting_a_closed_thread_deletes_every_reply_and_forgets_them() {
    let rig = Rig::discord();
    let (tab, _) = rig.claude_tab("work");
    let head = header(&rig, &tab);
    let reply = rig
        .block_on(post(&rig.cx, "🤖 a turn", Some(&head), PostOpts::default()))
        .unwrap();
    rig.block_on(super::homes::close_home(&rig.cx, &tab));
    rig.block_on(super::cleanup::delete_threads(&rig.cx, std::slice::from_ref(&head)));
    let deleted: Vec<String> = rig
        .discord
        .calls_to("DELETE", &format!("/channels/{DISCORD_DM}/messages/"))
        .into_iter()
        .map(|c| c.path.rsplit('/').next().unwrap().to_string())
        .collect();
    assert_eq!(deleted, [reply.clone(), head.clone()]);
    let chat = rig.discord_chat.clone().unwrap();
    assert_eq!(chat.thread_of(&reply), reply, "forgotten with its thread");
}
