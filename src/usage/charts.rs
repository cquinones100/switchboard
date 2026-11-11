//! What UX 2.0's Usage board draws (src/mission/usage_board.rs), worked
//! out off the server's task from files: when in the day each session
//! spent, which tab ran it, how fast each rate limit is filling, and the
//! request audit by hour and by what made the requests. New with board 24
//! of the UX 2.0 canvas ("Usage as charts"), with no tmux-version file
//! behind it.
//!
//! The statusline keeps one total a session a day (`spend-<session>`),
//! which says nothing of when it was spent, so it also appends each amount
//! it adds to the day's log, `usage/spent-<day>.tsv`, a line
//! `<epoch>\t<session>\t<dollars added>` (`log_spent`), kept `KEEP_DAYS`.
//! A session is tied to its tab by the survey of which pane runs which
//! Claude, which writes `usage/sessions-tabs.tsv`, a line
//! `<day>\t<session>\t<tab number>\t<tab name>\t<color>`, so a tab closed
//! since still has its number, name and color on the chart. Today's totals
//! stay the `spend-` files', so the board's total and the plain list's
//! agree; the log places them in time.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::sessions::{shares_today, usage_dir};
use crate::items::{Item, Target};
use crate::mux::TabId;
use crate::requests::Tally;
use crate::util;

/// Days of the spend log and the tabs' file kept: the week the board shows
/// and one more.
pub const KEEP_DAYS: i64 = 8;
/// The chart's step through today.
pub const QUARTER_MS: i64 = 15 * 60 * 1000;
const DAY_MS: i64 = 86_400_000;
/// How far into a window its pace is worth saying.
const PACE_AFTER_SECS: i64 = 15 * 60;
/// The fewest hours today's chart shows.
const MIN_HOURS: i64 = 4;
/// A rate is per hour of at least this much.
const RATE_FLOOR_HOURS: f64 = 0.25;

fn spent_file(state: &Path, day: &str) -> PathBuf {
    usage_dir(state).join(format!("spent-{day}.tsv"))
}

fn tabs_file(state: &Path) -> PathBuf {
    usage_dir(state).join("sessions-tabs.tsv")
}

/// What a spend file's change added today: its record's dollars now less
/// what they were, when it was the same day; nothing when they didn't grow.
pub fn added(old: &str, new: &str, today: &str) -> Option<f64> {
    let spent_of = |text: &str| -> f64 {
        let record = text.lines().last().unwrap_or("");
        let mut fields = record.split('\t');
        if fields.next() != Some(today) {
            return 0.0;
        }
        fields
            .next()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    let more = spent_of(new) - spent_of(old);
    (more > 0.000_05).then_some(more)
}

/// Appends what a session just spent to today's log; the first line of a
/// day prunes the logs past `KEEP_DAYS`.
pub fn log_spent(state: &Path, today: &str, session: &str, dollars: f64, now_secs: i64) {
    let file = spent_file(state, today);
    if !file.exists() {
        prune_spent(state, now_secs * 1000);
    }
    util::append_quietly(&file, format!("{now_secs}\t{session}\t{dollars:.4}\n"));
}

/// Removes the spend logs from before `KEEP_DAYS` ago.
pub fn prune_spent(state: &Path, now_ms: i64) {
    let oldest = util::ymd(&util::local_at_ms(now_ms - KEEP_DAYS * DAY_MS));
    let Ok(entries) = fs::read_dir(usage_dir(state)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(day) = name
            .strip_prefix("spent-")
            .and_then(|n| n.strip_suffix(".tsv"))
            && day.len() == 10
            && *day < *oldest
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// A tab as the survey saw it run a session: the tab while it is open, the
/// server process it is a tab of (its pid, which `switchboard upgrade`
/// keeps and a restart changes, since a restarted server may give a closed
/// tab's id to another), its number in its session, its name and its color
/// (`rrggbb`).
#[derive(Clone, Debug, PartialEq)]
pub struct TabSeen {
    pub session: String,
    pub tab: Option<u32>,
    pub server: u32,
    pub index: usize,
    pub name: String,
    pub color: Option<String>,
}

fn clean(text: &str) -> String {
    text.replace(['\t', '\n'], " ")
}

/// The tabs' file as it is: each session's latest line.
fn read_tabs(state: &Path) -> Vec<(String, TabSeen)> {
    let text = fs::read_to_string(tabs_file(state)).unwrap_or_default();
    let mut out: Vec<(String, TabSeen)> = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let (day, session, index, name, color, tab, server) = match f[..] {
            [day, session, index, name, color, tab, server] => (
                day,
                session,
                index,
                name,
                color,
                tab.parse().ok(),
                server.parse().unwrap_or(0),
            ),
            _ => continue,
        };
        let seen = TabSeen {
            session: session.to_string(),
            tab,
            server,
            index: index.parse().unwrap_or(0),
            name: name.to_string(),
            color: (!color.is_empty()).then(|| color.to_string()),
        };
        out.retain(|(_, s)| s.session != session);
        out.push((day.to_string(), seen));
    }
    out
}

/// Keeps which tab runs each session now, writing the file only when a
/// session is new to it today or its tab's name or color changed, and
/// leaving out the lines past `KEEP_DAYS`.
pub fn keep_tabs(state: &Path, now_ms: i64, open: &[TabSeen]) {
    let today = util::ymd(&util::local_at_ms(now_ms));
    let oldest = util::ymd(&util::local_at_ms(now_ms - KEEP_DAYS * DAY_MS));
    let mut kept = read_tabs(state);
    let before = kept.len();
    kept.retain(|(day, _)| *day >= oldest);
    let mut changed = kept.len() != before;
    // An open tab's earlier sessions take its number, name and color as
    // they are now, a Claude running in it or not, so it is shown so once
    // it closes.
    for seen in open.iter().filter(|s| s.tab.is_some() && s.server != 0) {
        for (_, k) in kept.iter_mut() {
            let same_tab = k.tab == seen.tab && k.server == seen.server;
            if same_tab && (k.index != seen.index || k.name != seen.name || k.color != seen.color) {
                k.index = seen.index;
                k.name = seen.name.clone();
                k.color = seen.color.clone();
                changed = true;
            }
        }
    }
    for seen in open.iter().filter(|s| !s.session.is_empty()) {
        let same = kept.iter().any(|(day, s)| {
            *day == today
                && s.session == seen.session
                && s.tab == seen.tab
                && s.server == seen.server
                && s.index == seen.index
                && s.name == seen.name
                && s.color == seen.color
        });
        if !same {
            kept.retain(|(_, s)| s.session != seen.session);
            kept.push((today.clone(), seen.clone()));
            changed = true;
        }
    }
    if !changed {
        return;
    }
    let text: String = kept
        .iter()
        .map(|(day, s)| {
            format!(
                "{day}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                clean(&s.session),
                s.index,
                clean(&s.name),
                s.color.as_deref().unwrap_or(""),
                s.tab.map(|t| t.to_string()).unwrap_or_default(),
                s.server
            )
        })
        .collect();
    let _ = util::write_atomic(tabs_file(state), text);
}

/// A tab's spend over a span, and its share of the span's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TabSpend {
    /// Which tab: `tab:<id>` while open, else its number and name.
    pub key: String,
    pub tab: Option<u32>,
    pub index: usize,
    pub name: String,
    pub color: Option<String>,
    pub spent: f64,
    pub share: f64,
    /// Its first spend the log has in the span, and its dollars an hour
    /// since.
    pub first_ms: Option<i64>,
    pub rate: Option<f64>,
}

/// A span's spend: the total, its bins (each tab's dollars, in `tabs`'
/// order), the first bin's start and each one's length.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpanSpend {
    pub total: f64,
    pub start_ms: i64,
    pub bin_ms: i64,
    pub bins: Vec<Vec<f64>>,
    pub tabs: Vec<TabSpend>,
}

/// A rate limit's window as the statusline last saw it, and where it ends
/// up at the pace it has filled so far.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Limit {
    pub key: String,
    pub name: String,
    pub used: f64,
    pub resets_at: i64,
    pub pace: Option<f64>,
    pub detail: Option<String>,
}

/// Everything the board draws.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub today: SpanSpend,
    pub week: SpanSpend,
    pub limits: Vec<Limit>,
    /// When the statusline last wrote the limits.
    pub limits_at: Option<i64>,
    /// The request audit's today and week, while it is on.
    pub requests: Option<(Tally, Tally)>,
    pub requests_file: String,
}

/// A window's length, for its pace.
fn window_secs(key: &str) -> Option<i64> {
    match key {
        "five_hour" => Some(5 * 3600),
        "seven_day" => Some(7 * 86400),
        _ => None,
    }
}

/// Where a window ends up by its reset at the pace it has filled: what is
/// used over the time gone, times the window; none in its first minutes.
pub fn pace(used: f64, window: i64, resets_at: i64, now: i64) -> Option<f64> {
    let gone = window - (resets_at - now);
    (gone >= PACE_AFTER_SECS && gone <= window).then(|| used * window as f64 / gone as f64)
}

fn limits(state: &Path, now: i64) -> Vec<Limit> {
    let limits = util::read_json(usage_dir(state).join("limits")).unwrap_or(Value::Null);
    let mut out = Vec::new();
    for (key, name) in [
        ("five_hour", "5-hour"),
        ("seven_day", "Week"),
        ("spend_limit", "Spend"),
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
        let detail = window["used_usd"]
            .as_f64()
            .map(|usd| format!("${usd:.2} of ${}", util::js_string(&window["limit_usd"])));
        out.push(Limit {
            key: key.into(),
            name: name.into(),
            used,
            resets_at,
            pace: window_secs(key).and_then(|w| pace(used, w, resets_at, now)),
            detail,
        });
    }
    out
}

/// A day's log: each line's time (ms), session and dollars.
fn read_log(state: &Path, day: &str) -> Vec<(i64, String, f64)> {
    let text = fs::read_to_string(spent_file(state, day)).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            let at: i64 = f.next()?.trim().parse().ok()?;
            let session = f.next()?.to_string();
            let dollars: f64 = f.next()?.trim().parse().ok()?;
            (dollars > 0.0).then_some((at * 1000, session, dollars))
        })
        .collect()
}

/// Which tab a session ran in: its open tab's, else the one the tabs' file
/// last saw, else one for the sessions no tab is known for.
fn tab_of(session: &str, open: &[TabSeen], kept: &[(String, TabSeen)]) -> TabSpend {
    if let Some(t) = open
        .iter()
        .find(|t| !t.session.is_empty() && t.session == session)
    {
        return TabSpend {
            key: t.tab.map_or_else(
                || format!("closed:{} {}", t.index, t.name),
                |id| format!("tab:{id}"),
            ),
            tab: t.tab,
            index: t.index,
            name: t.name.clone(),
            color: t.color.clone(),
            ..TabSpend::default()
        };
    }
    if let Some((_, t)) = kept.iter().find(|(_, t)| t.session == session) {
        // An earlier session of a tab still open is that tab's, renamed or
        // moved since: the same tab id of the same server process.
        let still_open = open
            .iter()
            .find(|o| o.tab.is_some() && o.tab == t.tab && o.server != 0 && o.server == t.server);
        if let Some(o) = still_open {
            return TabSpend {
                key: format!("tab:{}", o.tab.unwrap_or_default()),
                tab: o.tab,
                index: o.index,
                name: o.name.clone(),
                color: o.color.clone(),
                ..TabSpend::default()
            };
        }
        // A closed tab is one by its id and server, under its latest name
        // and color, however many sessions it ran or names it had.
        let Some(id) = t.tab.filter(|_| t.server != 0) else {
            return TabSpend {
                key: format!("closed:{} {}", t.index, t.name),
                index: t.index,
                name: t.name.clone(),
                color: t.color.clone(),
                ..TabSpend::default()
            };
        };
        let latest = kept
            .iter()
            .rev()
            .map(|(_, k)| k)
            .find(|k| k.tab == t.tab && k.server == t.server)
            .unwrap_or(t);
        return TabSpend {
            key: format!("closed:{}:{id}", t.server),
            index: latest.index,
            name: latest.name.clone(),
            color: latest.color.clone(),
            ..TabSpend::default()
        };
    }
    TabSpend {
        key: "other".into(),
        name: "sessions outside a tab".into(),
        ..TabSpend::default()
    }
}

/// A span's spend from each session's total and its logged amounts: the
/// totals by tab, the logged amounts in bins from `start_ms`.
#[allow(clippy::too_many_arguments)]
fn span(
    totals: &[(String, f64)],
    logged: &[(i64, String, f64)],
    start_ms: i64,
    bin_ms: i64,
    bins: usize,
    bin_of: &dyn Fn(i64) -> Option<usize>,
    now_ms: i64,
    tab: &dyn Fn(&str) -> TabSpend,
) -> SpanSpend {
    let mut tabs: Vec<TabSpend> = Vec::new();
    let at_of = |session: &str, tabs: &mut Vec<TabSpend>| -> usize {
        let t = tab(session);
        match tabs.iter().position(|x| x.key == t.key) {
            Some(i) => i,
            None => {
                tabs.push(t);
                tabs.len() - 1
            }
        }
    };
    for (session, spent) in totals {
        let i = at_of(session, &mut tabs);
        tabs[i].spent += spent;
    }
    let mut placed: Vec<(usize, usize, f64)> = Vec::new();
    let mut logged_sum: HashMap<usize, f64> = HashMap::new();
    for (at, session, dollars) in logged {
        let i = at_of(session, &mut tabs);
        tabs[i].first_ms = Some(tabs[i].first_ms.map_or(*at, |f: i64| f.min(*at)));
        *logged_sum.entry(i).or_insert(0.0) += dollars;
        if let Some(bin) = bin_of(*at) {
            let bin = bin.min(bins.saturating_sub(1));
            placed.push((bin, i, *dollars));
        }
    }
    // A tab's total is never less than what the log placed for it.
    for (i, sum) in logged_sum {
        tabs[i].spent = tabs[i].spent.max(sum);
    }
    // Sorted by spend, the bins' parts in the same order.
    let mut order: Vec<usize> = (0..tabs.len()).collect();
    order.sort_by(|a, b| tabs[*b].spent.total_cmp(&tabs[*a].spent));
    let place_of: HashMap<usize, usize> = order.iter().enumerate().map(|(p, i)| (*i, p)).collect();
    let tabs: Vec<TabSpend> = order.iter().map(|i| tabs[*i].clone()).collect();
    let mut grid = vec![vec![0.0; tabs.len()]; bins];
    for (bin, i, dollars) in placed {
        grid[bin][place_of[&i]] += dollars;
    }
    let total = tabs.iter().fold(0.0, |all, t| all + t.spent);
    let tabs = tabs
        .into_iter()
        .map(|mut t| {
            t.share = if total > 0.0 { t.spent / total } else { 0.0 };
            t.rate = t.first_ms.map(|first| {
                let hours = ((now_ms - first) as f64 / 3_600_000.0).max(RATE_FLOOR_HOURS);
                t.spent / hours
            });
            t
        })
        .collect::<Vec<_>>();
    SpanSpend {
        total,
        start_ms,
        bin_ms,
        bins: grid,
        tabs,
    }
}

/// The board's model at `now_ms`: today by quarter hour from the hour of
/// its first logged spend, the week by day, each tab's spend, share and
/// rate, the limits with their pace, and the requests when audited.
pub fn model(
    state: &Path,
    now_ms: i64,
    open: &[TabSeen],
    requests: Option<(Tally, Tally)>,
) -> Model {
    let now = util::local_at_ms(now_ms);
    let today = util::ymd(&now);
    let kept = read_tabs(state);
    let tab = |session: &str| tab_of(session, open, &kept);
    let shares = shares_today(state, &today);
    let log_today = read_log(state, &today);
    let first = log_today
        .iter()
        .map(|(at, _, _)| *at)
        .min()
        .unwrap_or(now_ms);
    // The hour as the local clock has it, and at least `MIN_HOURS` of the
    // day, so a morning's first quarter isn't the whole chart.
    let offset_ms = now.offset().seconds() as i64 * 1000;
    let midnight = now
        .start_of_day()
        .map(|z| z.timestamp().as_millisecond())
        .unwrap_or(now_ms - (now_ms + offset_ms).rem_euclid(DAY_MS));
    let first = first.min((now_ms - (MIN_HOURS - 1) * 3_600_000).max(midnight));
    let start = first - (first + offset_ms).rem_euclid(3_600_000);
    let bins = (((now_ms - start) / QUARTER_MS) as usize + 1).min(96);
    let quarter = |at: i64| (at >= start).then(|| ((at - start) / QUARTER_MS) as usize);
    let today_span = span(
        &shares, &log_today, start, QUARTER_MS, bins, &quarter, now_ms, &tab,
    );

    // The week by the calendar, so a day of 23 or 25 hours is one day.
    let date_back = |back: i64| now.date().checked_sub(jiff::Span::new().days(back)).ok();
    let ymd = |d: jiff::civil::Date| format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day());
    let week_start = date_back(6)
        .and_then(|d| d.to_zoned(now.time_zone().clone()).ok())
        .map(|z| z.timestamp().as_millisecond())
        .unwrap_or(midnight - 6 * DAY_MS);
    let today_date = now.date();
    let tz = now.time_zone().clone();
    let day_of = |at: i64| -> Option<usize> {
        let date = jiff::Timestamp::from_millisecond(at)
            .ok()?
            .to_zoned(tz.clone())
            .date();
        let back = (today_date - date).get_days();
        (0..7).contains(&back).then(|| (6 - back) as usize)
    };
    let mut week_log = Vec::new();
    let mut week_totals: HashMap<String, f64> = HashMap::new();
    for back in (1..7).rev() {
        let Some(day) = date_back(back) else {
            continue;
        };
        for (at, session, dollars) in read_log(state, &ymd(day)) {
            *week_totals.entry(session.clone()).or_insert(0.0) += dollars;
            week_log.push((at, session, dollars));
        }
    }
    // Today's bin holds today's totals, which the log may not have whole.
    let mut today_in_week = Vec::new();
    for (session, spent) in &shares {
        *week_totals.entry(session.clone()).or_insert(0.0) += spent;
        let first_today = log_today
            .iter()
            .filter(|(_, s, _)| s == session)
            .map(|(at, _, _)| *at)
            .min()
            .unwrap_or(midnight.max(now_ms - 1));
        today_in_week.push((first_today, session.clone(), *spent));
    }
    week_log.extend(today_in_week);
    let mut week_totals: Vec<(String, f64)> = week_totals.into_iter().collect();
    week_totals.sort_by(|a, b| a.0.cmp(&b.0));
    let week_span = span(
        &week_totals,
        &week_log,
        week_start,
        DAY_MS,
        7,
        &day_of,
        now_ms,
        &tab,
    );

    let limits_at = fs::metadata(usage_dir(state).join("limits"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64);
    Model {
        today: today_span,
        week: week_span,
        limits: limits(state, now_ms.div_euclid(1000)),
        limits_at,
        requests,
        requests_file: format!("requests/{today}.tsv"),
    }
}

/// Whether a row of the Usage view is one the cursor passes over (today's
/// total, a limit, the requests' total, what isn't counted).
pub fn is_still(item: &Item) -> bool {
    item.data.get("still").and_then(Value::as_bool) == Some(true)
}

/// The model the board draws, carried by the view's first row.
pub fn model_of(items: &[Item]) -> Option<Model> {
    let first = items.first()?;
    serde_json::from_value(first.data.get("model")?.clone()).ok()
}

/// The Usage view's rows with the board's model: the list's rows, the
/// total carrying the model, then a row per tab that spent this week
/// after it, today's first, and a row for each service with requests this
/// week but none today; what isn't a tab or a service passed over.
pub fn items(base: Vec<Item>, model: &Model) -> Vec<Item> {
    let week_only: Vec<Item> = model
        .requests
        .as_ref()
        .map(|(today, week)| {
            week.services
                .iter()
                .filter(|s| !today.services.iter().any(|t| t.service == s.service))
                .map(|s| {
                    Item::new(
                        format!("requests:{}", s.service),
                        format!("  {} 0", s.service),
                    )
                    .detail(format!("none today, {} this week", s.total))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut week_only = Some(week_only);
    let mut out = Vec::new();
    let mut tabs: Vec<&TabSpend> = model.today.tabs.iter().collect();
    for t in &model.week.tabs {
        if !tabs.iter().any(|x| x.key == t.key) {
            tabs.push(t);
        }
    }
    let mut tab_rows = Some(
        tabs.iter()
            .map(|t| {
                let today = model.today.tabs.iter().find(|x| x.key == t.key);
                let detail = match today {
                    Some(d) => format!(
                        "${:.2} today · {}% of the day",
                        d.spent,
                        (d.share * 100.0).round() as i64
                    ),
                    None => "nothing today".into(),
                };
                let row = Item::new(format!("tab:{}", t.key), format!("{} {}", t.index, t.name))
                    .detail(detail)
                    .data(serde_json::json!({ "tab_key": t.key }));
                match t.tab {
                    Some(id) => row.target(Target::Tab(TabId(id))),
                    None => row,
                }
            })
            .collect::<Vec<_>>(),
    );
    for mut item in base {
        if item.id == "requests:uncounted"
            && let Some(rows) = week_only.take()
        {
            out.extend(rows);
        }
        let is_service = item.id.starts_with("requests:") && item.id != "requests:uncounted";
        let mut data = match item.data.take() {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        if !is_service {
            data.insert("still".into(), Value::Bool(true));
        }
        let first = item.id == "spent";
        if first {
            data.insert("summary".into(), Value::Bool(true));
            data.insert(
                "model".into(),
                serde_json::to_value(model).unwrap_or(Value::Null),
            );
        }
        item.data = Value::Object(data);
        // The tabs after the total and the limits, so a long list of tabs
        // never leaves the limits below every row the cursor reaches.
        let past_the_limits = !first && !item.id.starts_with("limit:");
        if past_the_limits && let Some(rows) = tab_rows.take() {
            out.extend(rows);
        }
        out.push(item);
    }
    if let Some(rows) = tab_rows.take() {
        out.extend(rows);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sb-charts-{name}-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(usage_dir(&dir)).unwrap();
        dir
    }

    /// Today at `h:m` on this computer's clock, in milliseconds.
    fn at(h: i8, m: i8) -> i64 {
        util::local_now()
            .date()
            .at(h, m, 0, 0)
            .to_zoned(jiff::tz::TimeZone::system())
            .unwrap()
            .timestamp()
            .as_millisecond()
    }

    fn seen(session: &str, tab: Option<u32>, index: usize, name: &str) -> TabSeen {
        TabSeen {
            session: session.into(),
            tab,
            server: 1,
            index,
            name: name.into(),
            color: Some("23ef59".into()),
        }
    }

    #[test]
    fn a_spend_record_adds_only_what_grew_today() {
        let today = "2026-11-11";
        let old = format!("{today}\t1.5000\t1.5\t300\t2");
        let new = format!("{today}\t2.2500\t2.25\t300\t2");
        assert!((added(&old, &new, today).unwrap() - 0.75).abs() < 1e-9);
        assert_eq!(added(&new, &new, today), None);
        // Yesterday's record counts from nothing.
        let yesterday = "2026-11-10\t9.0000\t9\t300\t2";
        let first = format!("{today}\t0.5000\t9.5\t300\t2");
        assert!((added(yesterday, &first, today).unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(added("", "", today), None);
    }

    #[test]
    fn the_model_bins_today_by_quarter_hour_and_tab_with_each_tab_s_share_and_rate() {
        let state = temp("model");
        let now = at(12, 7);
        let today = util::ymd(&util::local_at_ms(now));
        let usage = usage_dir(&state);
        fs::write(usage.join("spend-a"), format!("{today}\t3.0000\t3\t1\t2")).unwrap();
        fs::write(usage.join("spend-b"), format!("{today}\t1.0000\t1\t2\t2")).unwrap();
        let log = [
            (at(10, 5), "a", 1.0),
            (at(10, 20), "b", 1.0),
            (at(11, 59), "a", 2.0),
        ];
        let text: String = log
            .iter()
            .map(|(ms, s, d)| format!("{}\t{s}\t{d:.4}\n", ms / 1000))
            .collect();
        fs::write(usage.join(format!("spent-{today}.tsv")), text).unwrap();
        // b's tab has closed: the tabs' file still names it.
        keep_tabs(&state, now, &[seen("b", Some(3), 3, "docs")]);
        let model = model(&state, now, &[seen("a", Some(7), 7, "api")], None);
        let day = &model.today;
        // From 9:00, four hours of the day at least, to the quarter now is in.
        assert_eq!(day.start_ms, at(9, 0));
        assert_eq!(day.bin_ms, QUARTER_MS);
        assert_eq!(day.bins.len(), 13);
        assert_eq!(day.tabs[0].key, "tab:7");
        assert_eq!(day.tabs[1].key, "closed:1:3");
        assert_eq!(day.tabs[1].name, "docs");
        assert_eq!(day.tabs[1].color.as_deref(), Some("23ef59"));
        assert_eq!(day.bins[4], vec![1.0, 0.0]);
        assert_eq!(day.bins[5], vec![0.0, 1.0]);
        assert_eq!(day.bins[11], vec![2.0, 0.0]);
        assert!((day.total - 4.0).abs() < 1e-9);
        assert!((day.tabs[0].share - 0.75).abs() < 1e-9);
        // $3 over the two hours and two minutes since 10:05.
        let rate = day.tabs[0].rate.unwrap();
        assert!((rate - 3.0 / (122.0 / 60.0)).abs() < 1e-6, "{rate}");
        // The week's last day holds today's totals.
        assert_eq!(model.week.bins.len(), 7);
        assert!((model.week.bins[6].iter().sum::<f64>() - 4.0).abs() < 1e-9);
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn an_earlier_session_of_a_tab_still_open_counts_as_that_tab() {
        let state = temp("earlier");
        let now = at(12, 7);
        let today = util::ymd(&util::local_at_ms(now));
        let usage = usage_dir(&state);
        fs::write(usage.join("spend-old"), format!("{today}\t1.0000\t1\t1\t2")).unwrap();
        fs::write(usage.join("spend-new"), format!("{today}\t2.0000\t2\t2\t2")).unwrap();
        // The tab ran "old" first, then "new", and is still open.
        keep_tabs(&state, now, &[seen("old", Some(7), 7, "api")]);
        let model = model(&state, now, &[seen("new", Some(7), 7, "api")], None);
        let tabs = &model.today.tabs;
        assert_eq!(tabs.len(), 1, "{tabs:?}");
        assert_eq!(tabs[0].key, "tab:7");
        assert!((tabs[0].spent - 3.0).abs() < 1e-9);
        // Its Claude has ended and the tab, renamed, stays open: still
        // that tab; a tab of another server with the same id isn't.
        keep_tabs(&state, now, &[seen("new", Some(7), 7, "api")]);
        let model = super::model(&state, now, &[seen("", Some(7), 7, "renamed")], None);
        assert_eq!(model.today.tabs.len(), 1, "{:?}", model.today.tabs);
        assert_eq!(model.today.tabs[0].name, "renamed");
        let other = TabSeen {
            server: 2,
            ..seen("", Some(7), 7, "api")
        };
        let model = super::model(&state, now, &[other], None);
        assert!(model.today.tabs.iter().all(|t| t.tab.is_none()));
        // Closed, its two sessions under two names are one tab, its latest.
        let model = super::model(&state, now, &[], None);
        let tabs = &model.today.tabs;
        assert_eq!(tabs.len(), 1, "{tabs:?}");
        assert_eq!(tabs[0].key, "closed:1:7");
        // Renamed while no Claude runs in it, then closed: its new name.
        keep_tabs(&state, now, &[seen("", Some(7), 7, "renamed")]);
        let model = super::model(&state, now, &[], None);
        assert_eq!(model.today.tabs[0].name, "renamed");
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn the_view_lists_the_total_and_the_limits_before_the_tabs() {
        let model = Model {
            today: SpanSpend {
                tabs: vec![TabSpend {
                    key: "tab:1".into(),
                    tab: Some(1),
                    index: 1,
                    name: "api".into(),
                    spent: 1.0,
                    share: 1.0,
                    ..TabSpend::default()
                }],
                ..SpanSpend::default()
            },
            ..Model::default()
        };
        let base = vec![
            Item::new("spent", "Spent today $1.00"),
            Item::new("limit:five_hour", "5-hour limit 10% used"),
            Item::new("requests", "Requests today 0"),
        ];
        let ids: Vec<String> = items(base, &model).into_iter().map(|i| i.id).collect();
        assert_eq!(ids, ["spent", "limit:five_hour", "tab:tab:1", "requests"]);
    }

    #[test]
    fn a_window_s_pace_is_where_it_ends_up_and_none_in_its_first_minutes() {
        let now = 1_000_000;
        // Two of five hours gone, 20% used: 50% by the reset.
        let pace_now = pace(20.0, 5 * 3600, now + 3 * 3600, now).unwrap();
        assert!((pace_now - 50.0).abs() < 1e-9);
        assert_eq!(pace(5.0, 5 * 3600, now + 5 * 3600 - 600, now), None);
    }

    #[test]
    fn the_tabs_file_is_written_only_when_a_session_or_its_tab_changes() {
        let state = temp("tabs");
        let now = util::now_ms();
        keep_tabs(&state, now, &[seen("a", Some(1), 1, "api")]);
        // A line the file never holds marks it: a rewrite would drop it.
        let file = tabs_file(&state);
        let mut text = fs::read_to_string(&file).unwrap();
        text.push_str("mark\n");
        fs::write(&file, &text).unwrap();
        keep_tabs(&state, now, &[seen("a", Some(1), 1, "api")]);
        assert!(fs::read_to_string(&file).unwrap().contains("mark"));
        keep_tabs(&state, now, &[seen("a", Some(1), 1, "api v2")]);
        let kept = read_tabs(&state);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].1.name, "api v2");
        let _ = fs::remove_dir_all(&state);
    }
}
