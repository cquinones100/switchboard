//! `switchboard tabs`: a line per pane of every tab, for a Claude session
//! to see what is in flight in the other tabs (Switchboard's part of
//! CLAUDE.md, /project-status). It replaces the one tmux call the tmux
//! version's CLAUDE.md gave, `tmux list-panes -a -f ... -F ...`, with the
//! same columns, tab-separated: the tab's number, the pane's place
//! (`<session>:@<tab>.%<pane>`), the tab's name as the tab bar shows it,
//! Claude's state there (blank for idle), the program in front, and the
//! directory. A pane parked behind a tool in its place is left out, as the
//! tmux call left it out, and so is everything when no server runs.

use std::process::ExitCode;

use serde_json::Value;

use crate::mux::socket;
use crate::util;

/// `switchboard tabs`.
pub fn main(_args: Vec<String>) -> ExitCode {
    let Ok(text) = socket::ask_ok(&["list"]) else {
        eprintln!("switchboard tabs: no Switchboard server is running");
        return ExitCode::from(1);
    };
    let list: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    print!("{}", lines(&list));
    ExitCode::SUCCESS
}

/// The lines, from the server's `list`.
pub fn lines(list: &Value) -> String {
    let text = |v: &Value| match v {
        Value::Null => String::new(),
        other => util::js_string(other),
    };
    let mut out = String::new();
    for session in list["sessions"].as_array().into_iter().flatten() {
        for tab in session["tabs"].as_array().into_iter().flatten() {
            for pane in tab["panes"].as_array().into_iter().flatten() {
                if pane["options"]["@pane_parked"] == "1" {
                    continue;
                }
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
                ];
                // A tab or newline in a name would break the columns.
                let row: Vec<String> = row.iter().map(|f| f.replace(['\t', '\n'], " ")).collect();
                out.push_str(&row.join("\t"));
                out.push('\n');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_line_per_pane_parked_ones_left_out() {
        let list = json!({ "sessions": [{ "name": "main", "tabs": [
            { "id": "@1", "index": 1, "label": "#12 ABC-1: Fix it", "panes": [
                { "id": "%1", "command": "2.1.0", "cwd": "/w/abc-1", "options": {}, "agent": { "state": "waiting" } },
                { "id": "%4", "command": "zsh", "cwd": "/w/abc-1", "options": { "@pane_parked": "1" }, "agent": null }
            ] },
            { "id": "@2", "index": 2, "label": "notes", "panes": [
                { "id": "%2", "command": "zsh", "cwd": "/home/me", "options": {}, "agent": null }
            ] }
        ] }] });
        assert_eq!(
            lines(&list),
            "1\tmain:@1.%1\t#12 ABC-1: Fix it\twaiting\t2.1.0\t/w/abc-1\n\
             2\tmain:@2.%2\tnotes\t\tzsh\t/home/me\n"
        );
    }
}
