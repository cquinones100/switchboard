//! Mission control's Sessions and Usage views. Ported from
//! lib/sidebar/sessions.js.
//!
//! Sessions: each Claude tab's session, how full its context is, its model,
//! how long it has run and how long since it last did anything, fullest
//! first, to tell which to compact or close. The context and model are what
//! Claude Code gives the statusline alone, which writes them to
//! `context/<session>` in the state directory (`<percent>\t<model>`); the age
//! is the session file's start, and the last activity its transcript's last
//! write.
//!
//! Usage: what Claude has cost today and how much of the subscription's rate
//! limits is used, which the statusline alone is told too: `usage/spend-
//! <session>` holds each session's share of a day (`<day>\t<spent>\t<cost
//! so far>\t<pid>`), and `usage/limits` the account's `rate_limits` as Claude
//! Code gives them. The cost is Claude Code's estimate at API prices, for
//! interactive sessions only (`claude -p` has no statusline), and the limits
//! are as the last session to draw saw them. Below them, with the
//! request_audit setting on, today's requests to services outside this
//! computer, as `requests::tally` counts them: the total by protocol, then
//! each service with its own protocols, busiest first.
//!
//! Both read `paths::state_dir()`. Today claude/statusline-command.sh still
//! writes these files under `~/.local/state/tmux-sidebar`, where the sidebar
//! daemon read them; the Rust statusline (`switchboard setup`'s, a later step
//! of the port) writes them here, and until then these views find nothing.
//!
//! Which pane runs which session is found from Claude Code's session files,
//! `~/.claude/sessions/<pid>.json`, by the pid's process group being the one
//! in front on a pane's terminal (`claudes`), read off the server's task.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::items::{Item, State, Target};
use crate::mux::procinfo;
use crate::mux::{PaneId, TabId};
use crate::requests::Tally;
use crate::util;

/// Context this full or more needs you: compact or start over.
pub const FULL: f64 = 80.0;
/// A rate limit this used or more needs you: slow down or wait for its reset.
pub const LIMIT_FULL: f64 = 80.0;

/// Where the statusline's files are, under a state directory.
pub fn context_dir(state: &Path) -> PathBuf {
    state.join("context")
}

pub fn usage_dir(state: &Path) -> PathBuf {
    state.join("usage")
}

/// How long ago, in a few characters: `<1m`, `5m`, `2h`, `3d`, as text.js's
/// `ago`; nothing for no time.
pub fn ago(seconds: i64, now_secs: i64) -> String {
    if seconds <= 0 {
        return String::new();
    }
    let s = (now_secs - seconds).max(0);
    if s < 60 {
        "<1m".into()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86400)
    }
}

// ── Which pane runs which session ───────────────────────────────────────────

/// A Claude session running in a pane, as its session file says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Claude {
    pub pid: u32,
    pub session_id: String,
    pub cwd: Option<PathBuf>,
    /// When it started, in milliseconds since the epoch.
    pub started_at: Option<i64>,
}

/// The interactive sessions' files (a headless `claude -p`'s are left out,
/// since a tab's session is its own Claude's), by pid.
pub fn session_files(dir: &Path) -> Vec<Claude> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| util::read_json(e.path()))
        .filter_map(|v| {
            let entry = v.get("entrypoint").and_then(Value::as_str).unwrap_or("");
            if entry.starts_with("sdk") {
                return None;
            }
            let pid = match v.get("pid") {
                Some(Value::Number(n)) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
                Some(Value::String(s)) => s.trim().parse().ok(),
                _ => None,
            }?;
            let session_id = v.get("sessionId").and_then(Value::as_str)?.to_string();
            if session_id.is_empty() {
                return None;
            }
            Some(Claude {
                pid,
                session_id,
                cwd: v.get("cwd").and_then(Value::as_str).map(PathBuf::from),
                started_at: v.get("startedAt").and_then(Value::as_f64).map(|t| t as i64),
            })
        })
        .collect()
}

/// Which pane each running session is in: its process leads the group in
/// front on the pane's terminal, or is a member of it (Claude started by
/// `claude-here` runs as a child of a Node script). `panes` gives each pane's
/// group in front. Blocking: a file per session and a read per pid.
pub fn claudes(panes: &[(PaneId, u32)], dir: &Path) -> HashMap<PaneId, Claude> {
    let by_group: HashMap<u32, PaneId> = panes.iter().map(|(p, g)| (*g, *p)).collect();
    let mut found: HashMap<PaneId, Claude> = HashMap::new();
    for claude in session_files(dir) {
        let pane = by_group
            .get(&claude.pid)
            .or_else(|| procinfo::group_of(claude.pid).and_then(|g| by_group.get(&g)));
        if let Some(pane) = pane {
            // A process that leads the group wins over one inside it.
            let leads = by_group.contains_key(&claude.pid);
            if leads || !found.contains_key(pane) {
                found.insert(*pane, claude);
            }
        }
    }
    found
}

// ── Sessions ────────────────────────────────────────────────────────────────

/// A tab running Claude, as the view needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeTab {
    pub tab: TabId,
    pub index: usize,
    pub name: String,
    /// Idle or done (finished), rather than working or waiting on you.
    pub quiet: bool,
    pub claude: Claude,
}

/// A session's context use and model, as the statusline last wrote them.
pub fn context_of(state: &Path, session: &str) -> (Option<f64>, Option<String>) {
    if !safe_id(session) {
        return (None, None);
    }
    let Some(text) = util::read_text(context_dir(state).join(session)) else {
        return (None, None);
    };
    let mut fields = text.trim_end_matches('\n').splitn(2, '\t');
    let used = fields.next().and_then(|u| u.trim().parse::<f64>().ok());
    let model = fields
        .next()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());
    (used.filter(|u| u.is_finite()), model)
}

/// A session id the statusline would have written a file for.
pub fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A session's transcript: in its directory's project, else in whichever
/// project has it (lib/sidebar/claude.js's `transcriptFor`).
pub fn transcript_of(claude_dir: &Path, claude: &Claude) -> Option<PathBuf> {
    let file = format!("{}.jsonl", claude.session_id);
    let projects = claude_dir.join("projects");
    if let Some(cwd) = &claude.cwd {
        let project: String = cwd
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let direct = projects.join(project).join(&file);
        if direct.exists() {
            return Some(direct);
        }
    }
    fs::read_dir(&projects)
        .ok()?
        .flatten()
        .map(|d| d.path().join(&file))
        .find(|f| f.exists())
}

fn last_write(claude_dir: &Path, claude: &Claude) -> Option<i64> {
    let file = transcript_of(claude_dir, claude)?;
    let modified = fs::metadata(file).ok()?.modified().ok()?;
    let secs = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(secs as i64)
}

/// The Sessions view's rows: fullest first, a full one needing you, a quiet
/// one idle.
pub fn session_items(
    tabs: &[ClaudeTab],
    state: &Path,
    claude_dir: &Path,
    now_ms: i64,
) -> Vec<Item> {
    let now = now_ms.div_euclid(1000);
    let mut rows: Vec<(f64, Item)> = tabs
        .iter()
        .map(|t| {
            let (used, model) = context_of(state, &t.claude.session_id);
            let started = t.claude.started_at.map(|ms| ms.div_euclid(1000));
            let quiet = if t.quiet {
                last_write(claude_dir, &t.claude)
            } else {
                None
            };
            let mut facts = vec![match used {
                Some(u) => format!("context {}%", u.round() as i64),
                None => "context unknown".into(),
            }];
            facts.extend(model.clone());
            if let Some(s) = started.filter(|s| *s > 0) {
                facts.push(format!("up {}", ago(s, now)));
            }
            if let Some(q) = quiet.filter(|q| *q > 0) {
                facts.push(format!("idle {}", ago(q, now)));
            }
            let state = if used.is_some_and(|u| u >= FULL) {
                State::Attention
            } else if t.quiet {
                State::Idle
            } else {
                State::Active
            };
            let item = Item::new(
                t.claude.session_id.clone(),
                format!("{} {}", t.index, t.name),
            )
            .detail(facts.join(" · "))
            .state(state)
            .target(Target::Tab(t.tab))
            .data(json!({ "used": used, "model": model, "session": t.claude.session_id }));
            (used.unwrap_or(-1.0), item)
        })
        .collect();
    // Stable, so sessions alike keep the tabs' order.
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    rows.into_iter().map(|(_, item)| item).collect()
}

// ── Usage ───────────────────────────────────────────────────────────────────

/// When a window resets: in so long today, else its weekday and time.
pub fn resets(at: i64, now: i64) -> String {
    let s = at - now;
    if s <= 0 {
        return "resets now".into();
    }
    if s < 86400 {
        let hours = s / 3600;
        let hours = if hours > 0 {
            format!("{hours}h ")
        } else {
            String::new()
        };
        return format!("resets in {hours}{}m", (s % 3600) / 60);
    }
    let when = util::local_at_ms(at * 1000);
    format!("resets {}", when.strftime("%a %-I:%M %p"))
}

/// Each session's share of `today`'s spend: its day, what it spent and its
/// cost so far.
pub fn shares_today(state: &Path, today: &str) -> Vec<(String, f64)> {
    let Ok(entries) = fs::read_dir(usage_dir(state)) else {
        return Vec::new();
    };
    let mut shares: Vec<(String, f64)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let session = name.strip_prefix("spend-")?.to_string();
            let text = fs::read_to_string(e.path()).ok()?;
            let mut fields = text.split('\t');
            let day = fields.next()?;
            let spent: f64 = fields.next()?.trim().parse().ok()?;
            (day == today && spent > 0.0).then_some((session, spent))
        })
        .collect();
    shares.sort_by(|a, b| a.0.cmp(&b.0));
    shares
}

/// What Claude has cost today, in dollars, as the Usage view sums it.
pub fn spent_today(state: &Path, now_ms: i64) -> f64 {
    let today = util::ymd(&util::local_at_ms(now_ms));
    shares_today(state, &today).iter().map(|(_, s)| s).sum()
}

/// `1,234`, as `toLocaleString("en-US")` writes a whole number.
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn by_count(counts: &indexmap::IndexMap<String, usize>) -> String {
    let mut list: Vec<(&String, &usize)> = counts.iter().collect();
    list.sort_by(|a, b| b.1.cmp(a.1));
    list.iter()
        .map(|(name, n)| format!("{name} {}", thousands(**n)))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The request audit's rows: the total by protocol, then each service.
pub fn request_items(day: &Tally) -> Vec<Item> {
    let mut rows = vec![
        Item::new(
            "requests",
            format!("Requests today {}", thousands(day.total)),
        )
        .detail(if day.total > 0 {
            format!("by protocol: {}", by_count(&day.protocols))
        } else {
            "none yet".into()
        }),
    ];
    for s in &day.services {
        rows.push(
            Item::new(
                format!("requests:{}", s.service),
                format!("  {} {}", s.service, thousands(s.total)),
            )
            .detail(by_count(&s.protocols)),
        );
    }
    rows.push(Item::new("requests:uncounted", "  Not counted").detail(
        "what Claude Code sessions reach themselves (their model and connectors), and skills' scripts",
    ));
    rows
}

/// The Usage view's rows: today's spend, each rate limit not yet reset, and
/// with `requests` the request audit's.
pub fn usage_items(state: &Path, now_ms: i64, requests: Option<&Tally>) -> Vec<Item> {
    let today = util::ymd(&util::local_at_ms(now_ms));
    let shares = shares_today(state, &today);
    let spent: f64 = shares.iter().map(|(_, s)| s).sum();
    let limits = util::read_json(usage_dir(state).join("limits")).unwrap_or(Value::Null);
    let now = now_ms.div_euclid(1000);
    let mut rows = vec![
        Item::new("spent", format!("Spent today ${spent:.2}")).detail(format!(
            "{} session{}, at API prices",
            shares.len(),
            if shares.len() == 1 { "" } else { "s" }
        )),
    ];
    for (key, name) in [
        ("five_hour", "5-hour limit"),
        ("seven_day", "Weekly limit"),
        ("spend_limit", "Spend limit"),
    ] {
        let window = &limits[key];
        let Some(used) = window["used_percentage"].as_f64().filter(|u| u.is_finite()) else {
            continue;
        };
        let Some(resets_at) = window["resets_at"].as_f64().map(|r| r as i64) else {
            continue;
        };
        if resets_at <= now {
            continue;
        }
        let mut detail = Vec::new();
        if let Some(usd) = window["used_usd"].as_f64() {
            detail.push(format!(
                "${usd:.2} of ${}",
                util::js_string(&window["limit_usd"])
            ));
        }
        detail.push(resets(resets_at, now));
        rows.push(
            Item::new(
                format!("limit:{key}"),
                format!("{name} {}% used", used.round() as i64),
            )
            .detail(detail.join(" · "))
            .state(if used >= LIMIT_FULL {
                State::Attention
            } else {
                State::Idle
            }),
        );
    }
    if let Some(day) = requests {
        rows.extend(request_items(day));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sb-sessions-{name}-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sessions_are_listed_by_how_full_their_context_is() {
        let state = temp("context");
        let claude_dir = state.join(".claude");
        fs::create_dir_all(context_dir(&state)).unwrap();
        fs::write(context_dir(&state).join("sess-full"), "86.4\tOpus").unwrap();
        fs::write(context_dir(&state).join("sess-light"), "12\tSonnet").unwrap();
        let now = util::now_ms();
        let hour = now - 3_600_000;
        let tab = |n: u32, name: &str, session: &str| ClaudeTab {
            tab: TabId(n),
            index: n as usize,
            name: name.into(),
            quiet: false,
            claude: Claude {
                pid: n,
                session_id: session.into(),
                cwd: Some("/work".into()),
                started_at: Some(hour),
            },
        };
        let items = session_items(
            &[tab(1, "light", "sess-light"), tab(2, "full", "sess-full")],
            &state,
            &claude_dir,
            now,
        );
        let rows: Vec<(String, String, State)> = items
            .into_iter()
            .map(|i| (i.title, i.detail, i.state))
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "2 full".to_string(),
                    "context 86% · Opus · up 1h".to_string(),
                    State::Attention
                ),
                (
                    "1 light".to_string(),
                    "context 12% · Sonnet · up 1h".to_string(),
                    State::Active
                ),
            ],
            "fullest first, a full one needing you"
        );
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn a_quiet_session_says_how_long_since_its_transcript_was_written() {
        let state = temp("quiet");
        let claude_dir = state.join(".claude");
        let project = claude_dir.join("projects/-work-a");
        fs::create_dir_all(&project).unwrap();
        let transcript = project.join("s1.jsonl");
        fs::write(&transcript, "{}\n").unwrap();
        let then = std::time::SystemTime::now() - std::time::Duration::from_secs(300);
        fs::File::options()
            .write(true)
            .open(&transcript)
            .unwrap()
            .set_modified(then)
            .unwrap();
        let tab = ClaudeTab {
            tab: TabId(4),
            index: 4,
            name: "quiet".into(),
            quiet: true,
            claude: Claude {
                pid: 1,
                session_id: "s1".into(),
                cwd: Some("/work/a".into()),
                started_at: None,
            },
        };
        let items = session_items(&[tab], &state, &claude_dir, util::now_ms());
        assert_eq!(items[0].detail, "context unknown · idle 5m");
        assert_eq!(items[0].state, State::Idle);
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn usage_starts_with_todays_spend_and_the_rate_limits() {
        let state = temp("usage");
        let dir = usage_dir(&state);
        fs::create_dir_all(&dir).unwrap();
        let now_ms = util::now_ms();
        let today = util::ymd(&util::local_at_ms(now_ms));
        // Each as the statusline leaves it: the day, what it spent today, its
        // cost so far and its pid.
        fs::write(dir.join("spend-a"), format!("{today}\t4.7500\t3\t300")).unwrap();
        fs::write(dir.join("spend-b"), format!("{today}\t1.0000\t1\t400")).unwrap();
        fs::write(dir.join("spend-c"), format!("{today}\t2.5000\t2.5\t500")).unwrap();
        fs::write(dir.join("spend-d"), format!("{today}\t21.5000\t1.5\t")).unwrap();
        // A session last used on another day counts for nothing today.
        fs::write(dir.join("spend-old"), "2020-01-01\t9\t3").unwrap();
        let now = now_ms.div_euclid(1000);
        fs::write(
            dir.join("limits"),
            json!({
                "five_hour": { "used_percentage": 42.4, "resets_at": now + 7_830 },
                "seven_day": { "used_percentage": 85, "resets_at": now + 3_030 },
                "spend_limit": { "used_percentage": 99, "resets_at": now - 5 },
            })
            .to_string(),
        )
        .unwrap();
        let rows: Vec<(String, String, State)> = usage_items(&state, now_ms, None)
            .into_iter()
            .map(|i| (i.title, i.detail, i.state))
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "Spent today $29.75".to_string(),
                    "4 sessions, at API prices".to_string(),
                    State::Idle
                ),
                (
                    "5-hour limit 42% used".to_string(),
                    "resets in 2h 10m".to_string(),
                    State::Idle
                ),
                (
                    "Weekly limit 85% used".to_string(),
                    "resets in 50m".to_string(),
                    State::Attention
                ),
            ],
            "a window that has reset is left out"
        );
        assert!((spent_today(&state, now_ms) - 29.75).abs() < 1e-9);
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn requests_are_listed_busiest_first_with_their_protocols() {
        let mut day = Tally::default();
        let mut add = |service: &str, protocol: &str| {
            day.total += 1;
            *day.protocols.entry(protocol.into()).or_insert(0) += 1;
            match day.services.iter_mut().find(|s| s.service == service) {
                Some(s) => {
                    s.total += 1;
                    *s.protocols.entry(protocol.into()).or_insert(0) += 1;
                }
                None => day.services.push(crate::requests::ServiceCount {
                    service: service.into(),
                    total: 1,
                    protocols: [(protocol.to_string(), 1)].into_iter().collect(),
                }),
            }
        };
        add("GitHub", "GraphQL");
        add("GitHub", "GraphQL");
        add("Slack", "MCP");
        add("Anthropic", "claude -p");
        add("constructor", "HTTPS");
        add("GitHub", "REST");
        add("Linear", "GraphQL");
        day.services.sort_by_key(|s| std::cmp::Reverse(s.total));
        let rows: Vec<String> = request_items(&day)
            .into_iter()
            .map(|i| format!("{} | {}", i.title, i.detail))
            .collect();
        assert_eq!(
            rows[0],
            "Requests today 7 | by protocol: GraphQL 3 · MCP 1 · claude -p 1 · HTTPS 1 · REST 1"
        );
        assert!(rows.contains(&"  GitHub 3 | GraphQL 2 · REST 1".to_string()));
        assert!(rows.contains(&"  constructor 1 | HTTPS 1".to_string()));
        assert!(rows.last().unwrap().starts_with("  Not counted |"));
        let state = temp("audit-off");
        assert!(
            !usage_items(&state, util::now_ms(), None)
                .iter()
                .any(|i| i.title.starts_with("Requests today")),
            "the setting off leaves them out"
        );
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn small_words_for_times_and_numbers() {
        assert_eq!(ago(0, 100), "");
        assert_eq!(ago(90, 100), "<1m");
        assert_eq!(ago(100, 100 + 3 * 3600), "3h");
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(999), "999");
        assert_eq!(resets(100, 200), "resets now");
        assert_eq!(resets(100 + 125, 100), "resets in 2m");
    }

    #[test]
    fn a_pane_runs_the_session_whose_process_leads_its_group() {
        let dir = temp("claudes");
        fs::write(
            dir.join("4242.json"),
            json!({ "pid": 4242, "sessionId": "abc-1", "cwd": "/w", "startedAt": 1000 })
                .to_string(),
        )
        .unwrap();
        fs::write(
            dir.join("4343.json"),
            json!({ "pid": 4343, "sessionId": "headless", "entrypoint": "sdk-cli" }).to_string(),
        )
        .unwrap();
        fs::write(dir.join("bad.json"), "not json").unwrap();
        let found = claudes(&[(PaneId(1), 4242), (PaneId(2), 4343)], &dir);
        assert_eq!(found.len(), 1, "a headless Claude is no tab's");
        assert_eq!(found[&PaneId(1)].session_id, "abc-1");
        assert_eq!(found[&PaneId(1)].started_at, Some(1000));
        let _ = fs::remove_dir_all(&dir);
    }
}
