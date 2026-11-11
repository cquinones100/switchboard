//! `switchboard tabs`: a line per pane of every tab, for a Claude session
//! to see what is in flight in the other tabs (Switchboard's part of Claude's
//! instructions, /project-status). It replaces the one tmux call the tmux
//! version's CLAUDE.md gave, `tmux list-panes -a -f ... -F ...`, with the
//! same columns, tab-separated: the tab's number, the pane's place
//! (`<session>:@<tab>.%<pane>`), the tab's name as the tab bar shows it,
//! Claude's state there (blank for idle), the program in front, and the
//! directory. A pane parked behind a tool in its place is left out, as the
//! tmux call left it out, and so is everything when no server runs.
//!
//! After those come what another Claude session needs to reach the pane's
//! Claude with `ListAgents` and `SendMessage`, blank where the pane runs
//! none: its session ID, its process ID, the name its `ListAgents` row
//! shows, and its address. Claude Code names a row by the session file's
//! `name` (`~/.claude/sessions/<pid>.json`, given or derived from the
//! directory or the first prompt, so the tabs of one checkout read alike)
//! with ` [ref]` after it, a hash of its socket cut as short as the other
//! rows allow, so the ref can't be worked out from one session alone. The
//! address, `uds:` and the session file's `messagingSocketPath`, is one
//! `SendMessage` takes as its `to` whatever the other rows are.
//! `--json` prints the same as an array of objects, for scripts.

use std::process::ExitCode;

use serde_json::{Value, json};

use crate::mux::socket;
use crate::util;

/// `switchboard tabs [--json]`.
pub fn main(args: Vec<String>) -> ExitCode {
    let as_json = match args.as_slice() {
        [] => false,
        [flag] if flag == "--json" => true,
        _ => {
            eprintln!("usage: switchboard tabs [--json]");
            return ExitCode::from(2);
        }
    };
    let Ok(text) = socket::ask_ok(&["list"]) else {
        eprintln!("switchboard tabs: no Switchboard server is running");
        return ExitCode::from(1);
    };
    let list: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if as_json {
        println!("{}", Value::Array(objects(&list)));
    } else {
        print!("{}", lines(&list));
    }
    ExitCode::SUCCESS
}

/// The columns' keys in `--json`, in the lines' order.
const KEYS: [&str; 10] = [
    "tab", "pane", "name", "state", "program", "dir", "session", "pid", "agent", "address",
];

/// A row per pane, its columns as text, from the server's `list`.
fn rows(list: &Value) -> Vec<[String; 10]> {
    let text = |v: &Value| match v {
        Value::Null => String::new(),
        other => util::js_string(other),
    };
    let mut out = Vec::new();
    for session in list["sessions"].as_array().into_iter().flatten() {
        for tab in session["tabs"].as_array().into_iter().flatten() {
            // The Notifications window is no tab.
            if tab["options"]["@notices_window"] == "1" {
                continue;
            }
            for pane in tab["panes"].as_array().into_iter().flatten() {
                if pane["options"]["@pane_parked"] == "1" {
                    continue;
                }
                // The process in front's, all four of the one session, even
                // while it carries a conversation on in a parked job.
                let claude = &pane["claude"]["peer"];
                let address = claude["socket"]
                    .as_str()
                    .map(|s| format!("uds:{s}"))
                    .unwrap_or_default();
                let row = [
                    text(&tab["index"]),
                    format!(
                        "{}:{}.{}",
                        text(&session["name"]),
                        text(&tab["id"]),
                        text(&pane["id"])
                    ),
                    text(&tab["label"]),
                    text(&pane["agent"]["state"]),
                    text(&pane["command"]),
                    text(&pane["cwd"]),
                    text(&claude["session"]),
                    text(&claude["pid"]),
                    text(&claude["name"]),
                    address,
                ];
                // A tab or newline in a name would break the columns.
                out.push(row.map(|f| f.replace(['\t', '\n'], " ")));
            }
        }
    }
    out
}

/// The lines, from the server's `list`.
pub fn lines(list: &Value) -> String {
    rows(list)
        .iter()
        .map(|row| format!("{}\n", row.join("\t")))
        .collect()
}

/// The same rows as objects keyed by `KEYS`, a blank column as null.
pub fn objects(list: &Value) -> Vec<Value> {
    rows(list)
        .into_iter()
        .map(|row| {
            let mut object = serde_json::Map::new();
            for (key, field) in KEYS.iter().zip(row) {
                let value = if field.is_empty() {
                    Value::Null
                } else {
                    json!(field)
                };
                object.insert((*key).to_string(), value);
            }
            Value::Object(object)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn list() -> Value {
        json!({ "sessions": [{ "name": "main", "tabs": [
            { "id": "@1", "index": 1, "label": "#12 ABC-1: Fix it", "panes": [
                { "id": "%1", "command": "2.1.0", "cwd": "/w/abc-1", "options": {}, "agent": { "state": "waiting" },
                  "claude": { "session": "job-1", "transcript": null,
                              "peer": { "session": "s-1", "pid": 4321, "name": "abc-1-81", "socket": "/tmp/cc-socks/4321.sock" } } },
                { "id": "%4", "command": "zsh", "cwd": "/w/abc-1", "options": { "@pane_parked": "1" }, "agent": null, "claude": null }
            ] },
            { "id": "@2", "index": 2, "label": "notes", "panes": [
                { "id": "%2", "command": "zsh", "cwd": "/home/me", "options": {}, "agent": null, "claude": null }
            ] }
        ] }] })
    }

    #[test]
    fn a_line_per_pane_parked_ones_left_out() {
        assert_eq!(
            lines(&list()),
            "1\tmain:@1.%1\t#12 ABC-1: Fix it\twaiting\t2.1.0\t/w/abc-1\ts-1\t4321\tabc-1-81\tuds:/tmp/cc-socks/4321.sock\n\
             2\tmain:@2.%2\tnotes\t\tzsh\t/home/me\t\t\t\t\n"
        );
    }

    #[test]
    fn json_names_the_same_columns() {
        let objects = objects(&list());
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0]["pid"], "4321");
        assert_eq!(objects[0]["agent"], "abc-1-81");
        assert_eq!(objects[0]["address"], "uds:/tmp/cc-socks/4321.sock");
        assert_eq!(objects[1]["dir"], "/home/me");
        assert_eq!(objects[1]["session"], Value::Null);
    }
}
