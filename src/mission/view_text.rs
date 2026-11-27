//! What a mission control view shows as text, for the commands that act on
//! what is in front (`views::Command`'s links, tabs named and media): with
//! mission control open they read the view rather than the tab's pane under
//! it. The chosen row's text comes first, then what its pane beside shows
//! (the Slack thread being read, which the server adds, since the threads
//! are kept apart from the items), then every other row's, newest first,
//! each with what it came from ("Slack · #general · Ada", "PR #12") for the
//! picker to show. Built from the items the view already holds, so it reads
//! no network and starts no process.

use serde_json::Value;

use super::views::{View, ViewId};
use crate::items::{Item, Target};
use crate::views::shown::{self, Shown};

/// A piece of what a view shows, with what it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Text {
    pub from: String,
    pub text: String,
}

/// What `item` came from, as the pickers name it: a Slack message by its
/// place and sender, a PR by its number, a ticket by its identifier, any
/// other row by its title.
pub fn from_of(id: &ViewId, item: &Item) -> String {
    let get = |k: &str| item.data.get(k).and_then(Value::as_str).unwrap_or("");
    match id {
        ViewId::Slack => slack_from(get("where"), get("from")),
        ViewId::Prs if item.data.pointer("/pr/number").is_some() => {
            let n = item.data.pointer("/pr/number").and_then(Value::as_u64);
            format!("PR #{}", n.unwrap_or_default())
        }
        ViewId::Tickets if !get("ticket").is_empty() => get("ticket").to_string(),
        _ => crate::ui::draw::cut(item.title.trim(), 40),
    }
}

/// A Slack message's source as the pickers name it.
pub fn slack_from(place: &str, who: &str) -> String {
    let mut parts = vec!["Slack"];
    // A DM's place is its sender, named once.
    let who = if who == place { "" } else { who };
    parts.extend([place, who].into_iter().filter(|s| !s.is_empty()));
    parts.join(" · ")
}

/// An item's text: its message (its lines, else its detail), what its
/// source keeps beside it (a PR's body, a ticket's description, a thread's
/// first message, a comment), the lines shown under it, and its link.
pub fn item_texts(id: &ViewId, item: &Item) -> Vec<Text> {
    let mut texts = Vec::new();
    let mut add = |from: &str, text: &str| {
        if !text.trim().is_empty() {
            texts.push(Text {
                from: from.to_string(),
                text: text.to_string(),
            });
        }
    };
    let from = from_of(id, item);
    let strs = |at: &str| -> Vec<String> {
        item.data
            .pointer(at)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    };
    if item.lines.is_empty() {
        add(&from, &item.detail);
    } else {
        add(&from, &item.lines.join("\n"));
    }
    // A Slack message's links behind words, which its text shows as the
    // words alone.
    for link in strs("/links") {
        add(&from, &link);
    }
    for at in ["/body", "/description", "/comment/body"] {
        if let Some(text) = item.data.pointer(at).and_then(Value::as_str) {
            add(&from, text);
        }
    }
    // The thread's first message, which heads a Slack row: its sender's.
    if let Some(root) = item.data.get("root").filter(|r| r.is_object()) {
        let get = |k: &str| item.data.get(k).and_then(Value::as_str).unwrap_or("");
        let who = root.get("from").and_then(Value::as_str).unwrap_or("");
        let root_from = slack_from(get("where"), who);
        if let Some(text) = root.get("text").and_then(Value::as_str) {
            add(&root_from, text);
        }
        for link in strs("/root/links") {
            add(&root_from, &link);
        }
    }
    add(&from, &item.more.join("\n"));
    if let Target::Url(url) = &item.target {
        add(&from, url);
    }
    if let Some(url) = item.data.get("url").and_then(Value::as_str) {
        add(&from, url);
    }
    // A Slack thread's row stands for every message of it the look found,
    // each its own sender's.
    if *id == ViewId::Slack && item.data.get("members").is_some() {
        for member in super::slack_board::members(item) {
            texts.extend(item_texts(id, &member));
        }
    }
    texts
}

impl View {
    /// What the view shows as text: the chosen row's (`cursor`), then
    /// `beside` (its pane beside's), then every other row's, newest first.
    pub fn texts(&self, cursor: Option<usize>, beside: Vec<Text>) -> Vec<Text> {
        let items = &self.data.items;
        let chosen = cursor.filter(|&at| at < items.len());
        let mut out: Vec<Text> = chosen
            .map(|at| item_texts(&self.id, &items[at]))
            .unwrap_or_default();
        out.extend(beside);
        let mut others: Vec<&Item> = items
            .iter()
            .enumerate()
            .filter(|(at, _)| Some(*at) != chosen)
            .map(|(_, item)| item)
            .collect();
        others.sort_by_key(|item| std::cmp::Reverse(item.time));
        for item in others {
            out.extend(item_texts(&self.id, item));
        }
        out
    }

    /// The links in what the view shows, in `texts`' order, each once, its
    /// text followed by what it came from.
    pub fn links(&self, cursor: Option<usize>, beside: Vec<Text>) -> Vec<Shown> {
        links_of(&self.texts(cursor, beside))
    }
}

/// The links in `texts`, in their order, each once, marked with what each
/// came from.
pub fn links_of(texts: &[Text]) -> Vec<Shown> {
    let mut out: Vec<Shown> = Vec::new();
    for t in texts {
        for link in shown::note_links(&t.text) {
            if out.iter().any(|s| s.url == link.url) {
                continue;
            }
            let text = if link.text.is_empty() {
                t.from.clone()
            } else {
                format!("{} · {}", link.text, t.from)
            };
            out.push(Shown {
                text,
                url: link.url,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::items::ViewData;

    fn slack(id: &str, time: i64, place: &str, who: &str, text: &str) -> Item {
        let mut item = Item::new(id, format!("DM {who}"))
            .time(time)
            .data(json!({ "where": place, "from": who }));
        item.lines = vec![text.to_string()];
        item
    }

    fn view(id: ViewId, items: Vec<Item>) -> View {
        View {
            id,
            title: String::new(),
            data: ViewData {
                items,
                ..ViewData::default()
            },
            loaded: true,
        }
    }

    #[test]
    fn the_slack_views_links_list_the_chosen_thread_first_then_the_others_newest_first() {
        let v = view(
            ViewId::Slack,
            vec![
                slack(
                    "a",
                    1,
                    "#general",
                    "Ada",
                    "old <https://a.example/old|old one>",
                ),
                slack("b", 3, "#dev", "Bo", "see https://b.example/new."),
                slack("c", 2, "Cy", "Cy", "chosen https://c.example/chosen"),
            ],
        );
        let thread = vec![Text {
            from: slack_from("Cy", "Di"),
            text: "a reply [the doc](https://d.example/doc)".into(),
        }];
        let links = v.links(Some(2), thread);
        let shown: Vec<(&str, &str)> = links
            .iter()
            .map(|s| (s.text.as_str(), s.url.as_str()))
            .collect();
        assert_eq!(
            shown,
            vec![
                ("Slack · Cy", "https://c.example/chosen"),
                ("the doc · Slack · Cy · Di", "https://d.example/doc"),
                ("Slack · #dev · Bo", "https://b.example/new"),
                ("Slack · #general · Ada", "https://a.example/old"),
            ]
        );
    }

    #[test]
    fn a_thread_rows_older_messages_are_read_each_its_senders() {
        let older = slack("o", 1, "#dev", "Ada", "spec at https://a.example/spec");
        let mut row = slack("n", 2, "#dev", "Bo", "agreed");
        row.data["members"] = json!([older, row.clone()]);
        let links = view(ViewId::Slack, vec![row]).links(None, Vec::new());
        assert_eq!(links[0].text, "Slack · #dev · Ada");
        assert_eq!(links[0].url, "https://a.example/spec");
    }

    #[test]
    fn a_threads_first_message_is_its_senders_not_the_replys() {
        let mut reply = slack("r", 1, "#dev", "Bo", "agreed");
        reply.data["root"] = json!({ "from": "Ada", "text": "see the spec", "links": ["[the spec](https://s.example/spec)"] });
        let links = view(ViewId::Slack, vec![reply]).links(Some(0), Vec::new());
        assert_eq!(links[0].text, "the spec · Slack · #dev · Ada");
    }

    #[test]
    fn a_prs_links_are_its_body_and_its_own_named_by_its_number() {
        let pr = Item::new("u", "#12 Fix it")
            .target(Target::Url("https://github.com/o/r/pull/12".into()))
            .data(json!({
                "pr": { "number": 12 },
                "url": "https://github.com/o/r/pull/12",
                "body": "Fixes [the bug](https://github.com/o/r/issues/3)",
            }));
        let links = view(ViewId::Prs, vec![pr]).links(Some(0), Vec::new());
        assert_eq!(
            links,
            vec![
                Shown {
                    text: "the bug · PR #12".into(),
                    url: "https://github.com/o/r/issues/3".into()
                },
                Shown {
                    text: "PR #12".into(),
                    url: "https://github.com/o/r/pull/12".into()
                },
            ]
        );
    }

    #[test]
    fn a_tickets_description_is_read_and_named_by_its_identifier() {
        let ticket = Item::new("ABC-1", "ABC-1 Do it").data(json!({
            "ticket": "ABC-1",
            "description": "Spec at https://docs.example/spec",
        }));
        let links = view(ViewId::Tickets, vec![ticket]).links(None, Vec::new());
        assert_eq!(links[0].text, "ABC-1");
        assert_eq!(links[0].url, "https://docs.example/spec");
    }
}
