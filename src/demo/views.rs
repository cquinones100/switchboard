//! What the demo lays out for the views that would otherwise be empty in
//! it, so each board of the design site has something to show: routines,
//! one whose last run failed for a tool it wasn't allowed (Routines, and
//! its "allow it and run again"); closed tabs and a past session (Reopen);
//! autopilot's mode, paused, so its board shows its queue and a landing
//! without opening a tab; and twenty notes opened lately (Notes' Recent
//! and its finder). `after_tabs` marks the open tabs once they are there:
//! a pinned tab, reminders, and a tab of autopilot's.
//!
//! Each is written in the format its own module reads (`routines::routine`,
//! `usage::closed`, `autopilot::mode`, `notes::store`), timed from when the
//! demo is laid out. The routines are paused, so a demo left running past
//! their time doesn't run one (its `claude -p` would be refused, putting a
//! failure in front of you); autopilot is paused, so it opens no tab.

use std::fs;
use std::path::Path;

use anyhow::Result;
use serde_json::{Value, json};

use crate::routines::routine::{self, Routine, WhenMissed};
use crate::routines::schedule::Schedule;
use crate::usage::closed::Closing;
use crate::util;

const HOUR_MS: i64 = 3_600_000;

/// Lays out what the views read, `now_ms` being when the demo is laid out.
pub fn lay_out(home: &Path, now_ms: i64) -> Result<()> {
    let state = home.join(".local/state/switchboard");
    routines(home, &state.join("routines"), now_ms)?;
    closed_tabs(home, &state, now_ms)?;
    past_session(home, now_ms)?;
    autopilot(home, &state, now_ms)?;
    recent_notes(home)?;
    Ok(())
}

fn routines(home: &Path, dir: &Path, now_ms: i64) -> Result<()> {
    fs::create_dir_all(dir)?;
    let now = util::local_at_ms(now_ms);
    let made = |id: &str,
                name: &str,
                when: &str,
                prompt: &str,
                tools: &[&str],
                needs: &[&str]|
     -> Result<Routine> {
        let schedule = Schedule::parse(when, &now).map_err(anyhow::Error::msg)?;
        Ok(Routine {
            id: id.into(),
            name: name.into(),
            dir: home.join("code/api").to_string_lossy().into_owned(),
            when: when.into(),
            schedule,
            when_missed: WhenMissed::Skip,
            prompt: prompt.into(),
            model: String::new(),
            tools: tools.iter().map(|t| t.to_string()).collect(),
            needs: needs.iter().map(|t| t.to_string()).collect(),
            paused: true,
            session: String::new(),
            session_dir: String::new(),
            handled: now_ms / 1000,
        })
    };
    let digest = made(
        "r-pr-digest",
        "Morning PR digest",
        "weekdays at 8:45",
        "List the PRs waiting on my review in acme/api and acme/web, oldest first, with what each changes in a line.",
        &["Bash(gh pr list:*)", "Bash(gh pr view:*)"],
        &[],
    )?;
    let triage = made(
        "r-ticket-triage",
        "Triage new tickets",
        "weekdays at 9:30",
        "Read the tickets opened in ABC since yesterday, and for each say which repository it belongs to and how big it looks.",
        &["Read", "Grep"],
        &["Bash(gh issue list:*)"],
    )?;
    for r in [&digest, &triage] {
        routine::save(dir, r)?;
    }
    // A run a day for the week before, the triage's last failing for the
    // tool it wasn't allowed.
    for back in (1..=6).rev() {
        let day = now_ms / 1000 - back * 86_400;
        let digest_run = format!(
            "{day}\t{}\t\t42\t0.08\tran\t4 PRs wait on you; the oldest is Move sessions to Redis\n",
            day + 2
        );
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("runs-r-pr-digest.tsv"))
            .and_then(|mut f| std::io::Write::write_all(&mut f, digest_run.as_bytes()))?;
        let (outcome, words) = if back == 1 {
            ("failed", "needs Bash(gh issue list:*)")
        } else {
            ("ran", "3 new tickets: two for acme/api, one for acme/web")
        };
        let triage_run = format!(
            "{}\t{}\t\t58\t0.11\t{outcome}\t{words}\n",
            day + 2700,
            day + 2702
        );
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("runs-r-ticket-triage.tsv"))
            .and_then(|mut f| std::io::Write::write_all(&mut f, triage_run.as_bytes()))?;
    }
    Ok(())
}

/// Two tabs closed earlier today, for Reopen.
fn closed_tabs(home: &Path, state: &Path, now_ms: i64) -> Result<()> {
    let closing = |at: i64, name: &str, path: &str, prompt: &str, outcome: &str| Closing {
        at,
        id: format!("@demo-closed-{}", at / 1000),
        index: 12,
        name: name.into(),
        kind: "closed".into(),
        path: Some(home.join(path)),
        lived: 2 * HOUR_MS,
        prompt: prompt.into(),
        outcome: outcome.into(),
        panes: 1,
        opened: at - 2 * HOUR_MS,
        ..Closing::default()
    };
    let lines = [
        closing(
            now_ms - 3 * HOUR_MS,
            "Trim the onboarding checklist",
            "code/web",
            "Take the steps nobody finishes out of the onboarding checklist",
            "changes asked",
        ),
        closing(
            now_ms - HOUR_MS,
            "Spike: a GraphQL gateway",
            "code/api",
            "Sketch a GraphQL gateway in front of the REST API, nothing to merge",
            "",
        ),
    ];
    let mut text = String::new();
    for c in lines {
        text.push_str(&serde_json::to_string(&c)?);
        text.push('\n');
    }
    util::write_atomic(state.join("closed.jsonl"), text)?;
    Ok(())
}

/// A session yesterday in the worktree no tab has, for Reopen's past
/// sessions.
fn past_session(home: &Path, now_ms: i64) -> Result<()> {
    let cwd = super::worktree(home, "abc-14-paginate-audit");
    let cwd = cwd.to_string_lossy().into_owned();
    let dir = home
        .join(".claude/projects")
        .join(crate::sidebar::claude::project_dir_name(&cwd));
    fs::create_dir_all(&dir)?;
    let id = "00000000-abc-4000-8000-000000000014";
    let at = |minutes: i64| {
        jiff::Timestamp::from_millisecond(now_ms - 26 * HOUR_MS + minutes * 60_000)
            .map(|t| t.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
            .unwrap_or_default()
    };
    let lines = [
        json!({ "type": "user", "cwd": cwd, "sessionId": id, "timestamp": at(0), "uuid": "demo-past-1",
            "message": { "role": "user", "content": "Paginate the audit log (ABC-14): fifty a page, newest first" } }),
        json!({ "type": "assistant", "cwd": cwd, "sessionId": id, "timestamp": at(9), "uuid": "demo-past-2",
            "message": { "id": "msg_demo_past", "role": "assistant", "model": "claude-demo",
                "content": [{ "type": "text", "text": "The audit log comes in pages of fifty, newest first, and the PR is in the merge queue." }] } }),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    util::write_atomic(dir.join(format!("{id}.jsonl")), text)?;
    Ok(())
}

/// Autopilot's mode on the demo note, paused, with a landing of today's.
fn autopilot(home: &Path, state: &Path, now_ms: i64) -> Result<()> {
    let mode = json!({
        "on": true,
        "paused": true,
        "note": "Demo Notes.md",
        "noteId": super::NOTE_ID,
        "repo": home.join("code/web").to_string_lossy(),
        "slots": 2,
        "cap": 5,
        "landed": [landing(now_ms)],
    });
    util::write_atomic(
        state.join("autopilot.json"),
        format!("{}\n", util::json_pretty(&mode)),
    )?;
    Ok(())
}

/// The made-up landing, an hour's work that landed an hour before
/// `now_ms`, or as soon after midnight as that allows, since the view
/// lists only today's landings.
fn landing(now_ms: i64) -> Value {
    let now = util::local_at_ms(now_ms);
    let midnight = now
        .start_of_day()
        .map_or(now_ms, |z| z.timestamp().as_millisecond());
    let at = (now_ms - HOUR_MS).max(midnight.min(now_ms));
    json!({
        // A tab's id and its `@autopilot` time, as autopilot keeps them
        // (`autopilot::view::times_of_closed` reads the time after the `@`).
        "id": format!("99@{}", (at - HOUR_MS) / 1000),
        "item": "BUG: Avatar upload fails on Safari",
        "note": super::NOTE_ID,
        "hash": "4f2a9c1",
        "at": at,
    })
}

/// Moves the made-up landing onto today once the day it landed on has
/// passed, so a demo left running past midnight still shows it; run with
/// the day's other data (`super::refresh_day`).
pub fn landing_today(home: &Path, now_ms: i64) -> Result<()> {
    let file = home.join(".local/state/switchboard/autopilot.json");
    let Some(mut mode) = util::read_json(&file) else {
        return Ok(());
    };
    let landed_at = mode["landed"][0]["at"].as_i64();
    if landed_at.is_none_or(|at| crate::autopilot::mode::same_day(at, now_ms)) {
        return Ok(());
    }
    mode["landed"] = json!([landing(now_ms)]);
    util::write_atomic(&file, format!("{}\n", util::json_pretty(&mode)))?;
    Ok(())
}

/// Twenty notes opened lately, so Notes' Recent shows eight and its finder
/// all twenty.
fn recent_notes(home: &Path) -> Result<()> {
    let notes = home.join("Notes");
    let titles = [
        "Release checklist",
        "Interview questions",
        "Team offsite",
        "Reading list",
        "API versioning ideas",
        "On-call handbook",
        "Quarterly goals",
        "Retro notes",
        "Design review feedback",
        "Database migration plan",
        "Customer calls",
        "Hiring plan",
        "Incident: login outage",
        "Pairing schedule",
        "Conference talk",
        "Book club",
        "Recipes",
    ];
    let mut recent: Vec<Value> = vec![json!("Demo Notes.md"), json!("Groceries.md")];
    for title in titles {
        let file = format!("{title}.md");
        util::write_atomic(notes.join(&file), format!("# {title}\n\n- [ ] \n"))?;
        recent.push(json!(file));
    }
    recent.push(json!("Demo Plans/Plan - Rate-limit the login endpoint.md"));
    util::write_atomic(
        notes.join(".notes.json"),
        format!(
            "{}\n",
            util::json_pretty(&json!({ "recent": recent, "last": "Demo Notes.md", "pinned": [] }))
        ),
    )?;
    Ok(())
}

/// Marks the open tabs, given each tab's name and id: a pinned tab, a
/// reminder on two, and a tab of autopilot's.
pub fn after_tabs(home: &Path, ids: &[(String, String)], now_ms: i64) -> Result<()> {
    let id_of = |name: &str| {
        ids.iter()
            .find(|(n, _)| n == name)
            .map(|(_, id)| id.clone())
    };
    let set = |name: &str, option: &str, value: &str| -> Result<()> {
        if let Some(id) = id_of(name) {
            super::ask(home, &["set", "-t", &id, option, value])?;
        }
        Ok(())
    };
    set("API quickstart guide", "@pinned", "1")?;
    let in_days = |days: i64| (now_ms / 1000 + days * 86_400).to_string();
    set("Shell", "@remind_at", &in_days(2))?;
    set("Shell", "@remind_note", "Check the release notes went out")?;
    set("Rate-limit the login endpoint", "@remind_at", &in_days(5))?;
    set(
        "Rate-limit the login endpoint",
        "@remind_note",
        "Ask Maya whether ten a minute is enough",
    )?;
    let opened = (now_ms / 1000 - 1800).to_string();
    for (option, value) in [
        ("@autopilot", opened.as_str()),
        ("@autopilot_item", "BUG: Password reset emails arrive late"),
        ("@autopilot_note", super::NOTE_ID),
        ("@autopilot_in", "1"),
    ] {
        set("Password reset email queue", option, value)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_ms(text: &str) -> i64 {
        let tz = jiff::tz::TimeZone::system();
        text.parse::<jiff::civil::DateTime>()
            .unwrap()
            .to_zoned(tz)
            .unwrap()
            .timestamp()
            .as_millisecond()
    }

    #[test]
    fn the_landing_is_today_even_just_after_midnight() {
        for now in [local_ms("2026-11-16T00:30"), local_ms("2026-11-16T14:00")] {
            let landed = landing(now);
            let at = landed["at"].as_i64().unwrap();
            assert!(at <= now, "{landed}");
            assert!(crate::autopilot::mode::same_day(at, now), "{landed}");
        }
    }

    #[test]
    fn a_landing_from_an_earlier_day_moves_onto_today_and_todays_stays() {
        let dir = std::env::temp_dir().join(format!("sb-views-{}", std::process::id()));
        let state = dir.join(".local/state/switchboard");
        fs::create_dir_all(&state).unwrap();
        let yesterday = local_ms("2026-11-15T14:00");
        autopilot(&dir, &state, yesterday).unwrap();
        let now = local_ms("2026-11-16T09:00");
        landing_today(&dir, now).unwrap();
        let mode = util::read_json(state.join("autopilot.json")).unwrap();
        let moved = mode["landed"][0]["at"].as_i64().unwrap();
        assert!(crate::autopilot::mode::same_day(moved, now), "{mode}");
        assert_eq!(mode["paused"], true);
        landing_today(&dir, now + 60_000).unwrap();
        let again = util::read_json(state.join("autopilot.json")).unwrap();
        assert_eq!(again["landed"][0]["at"].as_i64(), Some(moved));
        let _ = fs::remove_dir_all(&dir);
    }
}
