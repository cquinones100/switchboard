//! Meeting notes: each meeting of yours Fathom recorded becomes a note,
//! `Meetings/<title> <date>.md`, with its summary, its action items (yours
//! first) and its transcript, its timestamps linking back to the recording.
//! Ported from lib/sidebar/meetings.js. Fathom is reached through Claude's
//! connector, a headless Haiku allowed one Fathom call at a time: one list
//! every `MEETINGS_POLL_MS` during the working day, then a summary and a
//! transcript for each new meeting. A meeting is yours when you recorded it
//! or are among its invitees, by your Fathom email or your git name. A
//! meeting whose summary isn't ready yet is tried again on the next look;
//! one written, or whose note is already there, is remembered in
//! `meetings.json` in the state directory and never written again, so the
//! note is yours to edit. Only while the meeting_notes setting is on.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use jiff::Zoned;
use regex::Regex;
use serde_json::{Value, json};

use super::calendar::CalEvent;
use super::{Connector, Dirs, new_notes, write_new};
use crate::util;

pub const MEETINGS_POLL_MS: i64 = 30 * 60_000;
// For the half hour after a meeting on your calendar ends, a look every few
// minutes, since Fathom has its summary a few minutes after the call.
const AFTER_MEETING_MS: i64 = 30 * 60_000;
const AFTER_MEETING_POLL_MS: i64 = 5 * 60_000;
// Looked for only between these hours, on any day.
const MEETINGS_HOURS: (i8, i8) = (7, 21);
// The first look goes back this far.
const FIRST_LOOK_DAYS: i64 = 2;
const FATHOM: &str = "mcp__claude_ai_Fathom__";

/// An action item of a meeting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Action {
    pub done: bool,
    pub text: String,
    pub who: String,
    pub link: Option<String>,
}

/// A meeting as Fathom lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meeting {
    pub title: String,
    pub date: String,
    pub id: u64,
    pub url: String,
    pub recorded_by: String,
    pub invitees: Vec<String>,
    pub actions: Vec<Action>,
}

/// `list_meetings`' text, a meeting a line, its summary and action items
/// indented under it:
///
/// ```text
/// - Title | 2026-10-28 | id: 1 | url: https://… | recorded by a@b.c | Ann, Bo
///   Action items:
///     • [ ] Do it — assigned to Ann (https://…?timestamp=12.9)
/// ```
pub fn meetings_from(text: &str) -> Vec<Meeting> {
    static HEAD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^- (.*) \| (\d{4}-\d{2}-\d{2}) \| id: (\d+) \| url: (\S+) \| recorded by (.*?)(?: \| (.*))?$").unwrap()
    });
    static ACTION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\s+• \[( |x)\] (.*?)(?: — assigned to (.*?))?(?: \((https://\S+)\))?$")
            .unwrap()
    });
    let mut meetings: Vec<Meeting> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(h) = HEAD.captures(line) {
            meetings.push(Meeting {
                title: h[1].to_string(),
                date: h[2].to_string(),
                id: h[3].parse().unwrap_or(0),
                url: h[4].to_string(),
                recorded_by: h[5].to_string(),
                invitees: h
                    .get(6)
                    .map(|m| {
                        m.as_str()
                            .split(", ")
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                actions: Vec::new(),
            });
        } else if let Some(a) = ACTION.captures(line)
            && let Some(m) = meetings.last_mut()
        {
            m.actions.push(Action {
                done: &a[1] == "x",
                text: a[2].to_string(),
                who: a.get(3).map(|w| w.as_str().to_string()).unwrap_or_default(),
                link: a.get(4).map(|l| l.as_str().to_string()),
            });
        }
    }
    meetings
}

/// Whether a meeting is yours: recorded by or inviting any of `me`.
pub fn is_mine(meeting: &Meeting, me: &[String]) -> bool {
    me.is_empty()
        || std::iter::once(&meeting.recorded_by)
            .chain(&meeting.invitees)
            .any(|who| {
                let who = who.to_lowercase();
                me.iter().any(|m| who.contains(&m.to_lowercase()))
            })
}

// `1:05` from a link's `timestamp=65.9`.
fn timestamp(link: Option<&str>) -> String {
    static AT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"timestamp=([\d.]+)").unwrap());
    link.and_then(|l| AT.captures(l))
        .and_then(|c| c[1].parse::<f64>().ok())
        .map(|s| {
            let s = s.floor() as i64;
            format!("{}:{:02}", s / 60, s % 60)
        })
        .unwrap_or_default()
}

/// Where a meeting's note goes.
pub fn meeting_file(notes: &Path, meeting: &Meeting) -> PathBuf {
    static LEAD_DOTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\.+").unwrap());
    let title = meeting.title.replace(['/', ':', '\\'], "-");
    let title = LEAD_DOTS.replace(&title, "");
    notes
        .join("Meetings")
        .join(format!("{} {}.md", title.trim(), meeting.date))
}

/// The note, from what Fathom gave alone. Fathom links every line of a
/// summary to its moment in the recording; the note keeps the words, and
/// the transcript keeps its timestamps' links.
pub fn meeting_note(meeting: &Meeting, summary: &str, transcript: &str, me: &[String]) -> String {
    static LINKED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\[((?:[^\[\]]|\[[^\]]*\])*)\]\(https://fathom\.video/[^)]*\)").unwrap()
    });
    static PURPOSE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^## Meeting Purpose\n+").unwrap());
    static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^(#{2,3}) ").unwrap());
    static INDENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^ {2}").unwrap());
    static PARENS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" \([^)]*\)$").unwrap());
    let plain = LINKED.replace_all(summary, "$1");
    let plain = PURPOSE.replace(&plain, "");
    let plain = HEADING.replace_all(&plain, "#$1 ");
    let plain = INDENT.replace_all(&plain, "");
    let plain = plain.trim();
    // Yours when the assignee is you by whole name (what follows it in
    // parentheses, their pronouns or email, left out) or by an email Fathom
    // gives with it, in parentheses or not, never by part of a name: a git
    // name of "Ann" isn't Joanne's.
    let mine = |a: &Action| {
        let who = a.who.to_lowercase();
        let name = PARENS.replace(&who, "").trim().to_string();
        me.iter().any(|m| {
            let m = m.to_lowercase();
            name == m
                || (m.contains('@')
                    && who
                        .split(|c: char| c.is_whitespace() || "<>(),".contains(c))
                        .any(|w| w == m))
        })
    };
    let actions: Vec<String> = meeting
        .actions
        .iter()
        .filter(|a| mine(a))
        .chain(meeting.actions.iter().filter(|a| !mine(a)))
        .map(|a| {
            format!(
                "- {}{}{}{}",
                if mine(a) {
                    format!("[{}] ", if a.done { "x" } else { " " })
                } else {
                    String::new()
                },
                a.text,
                if a.who.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", PARENS.replace(&a.who, ""))
                },
                a.link
                    .as_ref()
                    .map(|l| format!(" [{}]({l})", timestamp(Some(l))))
                    .unwrap_or_default()
            )
        })
        .collect();
    let mut lines = vec![
        format!("# {} {}", meeting.title, meeting.date),
        String::new(),
        format!(
            "[Recording]({}) · recorded by {}{}",
            meeting.url,
            meeting.recorded_by,
            if meeting.invitees.is_empty() {
                String::new()
            } else {
                format!(" · {}", meeting.invitees.join(", "))
            }
        ),
        String::new(),
        "## Action items".into(),
        String::new(),
    ];
    if actions.is_empty() {
        lines.push("None.".into());
    } else {
        lines.extend(actions);
    }
    lines.extend([
        String::new(),
        "## Summary".into(),
        String::new(),
        if plain.is_empty() {
            "None.".into()
        } else {
            plain.to_string()
        },
        String::new(),
        "## Transcript".into(),
        String::new(),
    ]);
    for line in transcript.trim().split('\n').filter(|l| !l.is_empty()) {
        lines.push(line.to_string());
        lines.push(String::new());
    }
    format!("{}\n", lines.join("\n").trim_end())
}

/// Whether to look now: between seven and nine, every `MEETINGS_POLL_MS`,
/// or every `AFTER_MEETING_POLL_MS` for the half hour after a meeting on
/// today's calendar (`events`) ended.
pub fn meetings_due(now: &Zoned, last: i64, events: &[CalEvent]) -> bool {
    let tz = now.time_zone();
    let at = now.timestamp().as_millisecond();
    let just_ended = events.iter().any(|e| {
        !e.all_day
            && e.ends(tz)
                .map(|t| t.as_millisecond())
                .is_some_and(|end| end <= at && at - end < AFTER_MEETING_MS)
    });
    now.hour() >= MEETINGS_HOURS.0
        && now.hour() < MEETINGS_HOURS.1
        && at - last
            >= if just_ended {
                AFTER_MEETING_POLL_MS
            } else {
                MEETINGS_POLL_MS
            }
}

async fn git_name() -> Option<String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["config", "--global", "user.name"]);
    let out = util::run_async(&mut cmd, Some(Duration::from_secs(10)))
        .await
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

/// Looks for new meetings and writes their notes; gives the lines for the
/// log.
pub async fn write_meetings(
    dirs: &Dirs,
    now: &Zoned,
    call: &impl Connector,
) -> anyhow::Result<Vec<String>> {
    let state_file = dirs.state.join("meetings.json");
    let mut state = util::read_json_object(&state_file);
    let mut log = Vec::new();
    let mut me: Vec<String> = state
        .get("me")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    if !state.contains_key("me") {
        let identity = call
            .call(&format!("{FATHOM}get_identity"), json!({}), None)
            .await?;
        let identity: Value = serde_json::from_str(if identity.trim().is_empty() {
            "{}"
        } else {
            &identity
        })
        .unwrap_or(Value::Null);
        for who in [
            identity
                .get("email")
                .and_then(Value::as_str)
                .map(str::to_string),
            identity
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string),
            git_name().await,
        ]
        .into_iter()
        .flatten()
        {
            if !who.is_empty() && !me.contains(&who) {
                me.push(who);
            }
        }
        state.insert("me".into(), json!(me));
    }
    let mut written: Vec<u64> = state
        .get("written")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let at = now.timestamp();
    let since = state
        .get("since")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            super::ical::iso(at - jiff::SignedDuration::from_hours(24 * FIRST_LOOK_DAYS))
        });
    let listed = meetings_from(
        &call
            .call(
                &format!("{FATHOM}list_meetings"),
                json!({ "created_after": since, "include_action_items": true, "max_pages": 3 }),
                None,
            )
            .await?,
    );
    static NOT_READY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)no summary|not (yet )?available|processing").unwrap());
    let mut waiting = false;
    let fresh: Vec<&Meeting> = listed
        .iter()
        .filter(|m| is_mine(m, &me) && !written.contains(&m.id))
        .collect();
    for meeting in fresh {
        let file = meeting_file(&dirs.notes, meeting);
        if !file.exists() {
            let summary = call
                .call(
                    &format!("{FATHOM}get_meeting_summary"),
                    json!({ "recording_id": meeting.id }),
                    None,
                )
                .await?;
            let head: String = summary.chars().take(200).collect();
            if summary.trim().is_empty() || NOT_READY.is_match(&head) {
                waiting = true;
                continue;
            }
            let transcript = call
                .call(
                    &format!("{FATHOM}get_meeting_transcript"),
                    json!({ "recording_id": meeting.id, "url": meeting.url }),
                    Some(Duration::from_secs(300)),
                )
                .await?;
            if write_new(&file, &meeting_note(meeting, &summary, &transcript, &me))? {
                log.push(format!(
                    "meetings: wrote {}",
                    file.file_name().unwrap_or_default().to_string_lossy()
                ));
                new_notes::note_written(&dirs.state, &dirs.notes, &file, "Fathom");
            }
        }
        written.push(meeting.id);
        let skip = written.len().saturating_sub(500);
        written.drain(..skip);
    }
    // Look from now on, less a day for meetings Fathom lists late, unless
    // one is still waiting on its summary.
    if !waiting {
        state.insert(
            "since".into(),
            json!(super::ical::iso(at - jiff::SignedDuration::from_hours(24))),
        );
    }
    state.insert("written".into(), json!(written));
    util::write_atomic(&state_file, util::json_compact(&Value::Object(state)))?;
    Ok(log)
}
