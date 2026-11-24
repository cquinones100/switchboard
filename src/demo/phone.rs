//! The phone app's made-up tabs, for `switchboard phone preview`
//! (`phone::preview`): Claude tabs in every group the sidebar has, as the
//! server's `watch` lists them (`phone::serve`'s `phone_snapshot`), each
//! made a sidebar tab and placed, marked and worded by the same
//! `phone::drawer` the server's go through, and a few turns each, as
//! `phone::turns` reads them from a transcript. One waits on a question
//! with choices, one on a question that takes several, one finished with a
//! long reply in Markdown, one finished on a failure, one with images you
//! sent and a before and after its Claude showed with `look`, one with a
//! long conversation its channel loads as it is scrolled up; the rest
//! work, watch, loop, land or sit idle. Every name is made up, in the
//! demo's style (`acme/` repositories, people by first name).

use serde_json::{Value, json};

use crate::facts::{Checks, Landing, Method, PrState};
use crate::items::{Item, State, ViewData};
use crate::mission::views::{View, ViewId, header};
use crate::mux::TabId;
use crate::phone::views::{self, Kind};
use crate::sidebar::claude::{LoopRun, Scheduled, SessionRef};
use crate::sidebar::model::{MergeState, Rules, TabLoop, TabModel, TabState};

/// A made-up tab: its pane's number, name, repository, branch, state and
/// what it is doing, what puts it in its group beyond its state (`shape`,
/// given now in ms), and its turns, oldest first, each `(who, text, images,
/// minutes ago)`.
struct Tab {
    pane: u32,
    name: &'static str,
    repo: &'static str,
    branch: &'static str,
    state: &'static str,
    wait: Option<&'static str>,
    minutes_ago: i64,
    prompt: Option<fn() -> Value>,
    shape: fn(&mut TabModel, i64),
    turns: &'static [(&'static str, &'static str, &'static [&'static str], i64)],
}

const LONG_REPLY: &str = "## What changed\n\nThe rate limiter now keys on the **API token** rather than the client's address, so two services behind one proxy no longer share a budget.\n\n1. `limits.rs` reads the token from the `Authorization` header.\n2. A request with no token falls back to the address, as before.\n3. The budget is kept in Redis under `limit:<token>`, expiring after a minute.\n\n## Tests\n\n- `limits::a_token_has_its_own_budget`\n- `limits::no_token_falls_back_to_the_address`\n- `limits::the_budget_resets_after_a_minute`\n\nAll 214 tests pass. The load test held 1,200 requests a second with the p99 at 41 ms, against 38 ms before.\n\n```rust\nlet key = token.map_or_else(|| format!(\"limit:{addr}\"), |t| format!(\"limit:{t}\"));\nlet budget = Budget::new(settings.requests_per_minute.unwrap_or(DEFAULT_REQUESTS_PER_MINUTE), Duration::from_secs(settings.window_seconds.unwrap_or(60)), redis.clone(), metrics.counter(\"rate_limit\"));\n```\n\n| Route | Before | After | p99 | Budget key |\n| --- | --- | --- | --- | --- |\n| `/v1/orders/{order_id}/line_items` | 38 ms | 41 ms | 41 ms | `limit:<token>` |\n| `/v1/search?q=` | 22 ms | 23 ms | 25 ms | `limit:<address>` |\n\n## Left for you\n\nThe dashboards still group by address: Sam owns them, so I left a note in the PR rather than changing them.";

const TABS: &[Tab] = &[
    Tab {
        pane: 11,
        name: "Migrate the orders table",
        repo: "api",
        branch: "abc-142-orders-migration",
        state: "waiting",
        wait: Some("Bash: run the migration"),
        minutes_ago: 2,
        prompt: Some(migration_prompt),
        shape: plain,
        turns: &[
            (
                "you",
                "Move the orders table to the new schema and run it on staging.",
                &[],
                14,
            ),
            ("tool", "Read migrations/0042_orders.sql", &[], 13),
            ("tool", "Edit migrations/0043_orders_v2.sql", &[], 9),
            (
                "claude",
                "The migration is written: it copies the rows in batches of 5,000 and swaps the tables in one transaction. Ready to run it on staging.",
                &[],
                3,
            ),
        ],
    },
    Tab {
        pane: 12,
        name: "Release checklist",
        repo: "web",
        branch: "abc-150-release",
        state: "waiting",
        wait: Some("AskUserQuestion"),
        minutes_ago: 6,
        prompt: Some(checks_prompt),
        shape: plain,
        turns: &[
            ("you", "Get the release branch ready to merge.", &[], 30),
            (
                "claude",
                "The branch is up to date with main. Before I merge it, which checks should run?",
                &[],
                6,
            ),
        ],
    },
    Tab {
        pane: 13,
        name: "Rate limit by token",
        repo: "api",
        branch: "abc-131-rate-limit",
        state: "done",
        wait: None,
        minutes_ago: 4,
        prompt: None,
        shape: plain,
        turns: &[
            (
                "you",
                "Make the rate limiter key on the API token rather than the address.",
                &[],
                52,
            ),
            ("tool", "Edit src/limits.rs", &[], 40),
            ("tool", "Bash: cargo test", &[], 12),
            (
                "tool",
                "Bash: REDIS_URL=redis://127.0.0.1:6379/2 RATE_LIMIT_REQUESTS_PER_MINUTE=1200 RATE_LIMIT_WINDOW_SECONDS=60 cargo run --release --bin load-test -- --target http://127.0.0.1:8080/v1/orders/ord_7f3a9c2e/line_items --concurrency 64 --duration 120s --header \"Authorization: Bearer test_token_for_the_load_test\" --report target/load-test/rate-limit-by-token.json",
                &[],
                8,
            ),
            ("claude", LONG_REPLY, &[], 4),
        ],
    },
    Tab {
        pane: 14,
        name: "Deploy the docs site",
        repo: "docs",
        branch: "abc-160-deploy",
        state: "done",
        wait: None,
        minutes_ago: 9,
        prompt: None,
        shape: plain,
        turns: &[
            ("you", "Deploy the docs site to production.", &[], 20),
            ("tool", "Bash: npm run deploy", &[], 11),
            (
                "tool",
                "Bash failed: Error: ENOENT: no such file or directory, open 'dist/sitemap.xml'",
                &[],
                10,
            ),
            (
                "claude",
                "API Error: 529 Overloaded. The deploy stopped after the build failed: dist/sitemap.xml was never written, since the sitemap step is skipped when NODE_ENV isn't set. Nothing was published.",
                &[],
                9,
            ),
        ],
    },
    Tab {
        pane: 15,
        name: "Checkout page layout",
        repo: "web",
        branch: "abc-155-checkout",
        state: "done",
        wait: None,
        minutes_ago: 25,
        prompt: None,
        shape: plain,
        turns: &[
            (
                "you",
                "The total wraps under the button on a small phone, see these.",
                &["demo-checkout.svg", "demo-total.svg"],
                40,
            ),
            ("tool", "Edit src/checkout/Summary.tsx", &[], 33),
            (
                "tool",
                "Bash: switchboard look --before-after before.png after.png",
                &[],
                26,
            ),
            ("look", "", &["demo-before.svg", "demo-after.svg"], 26),
            (
                "claude",
                "Fixed: the total and the button sit in one row that wraps the button, not the total, under 360 pixels.",
                &[],
                25,
            ),
        ],
    },
    Tab {
        pane: 16,
        name: "Search results paging",
        repo: "api",
        branch: "abc-138-paging",
        state: "working",
        wait: None,
        minutes_ago: 1,
        prompt: None,
        shape: |t, _| t.activity = Some("Run the search tests".into()),
        turns: &[
            (
                "you",
                "Page the search results by cursor instead of offset.",
                &[],
                18,
            ),
            ("tool", "Edit src/search/page.rs", &[], 7),
        ],
    },
    Tab {
        pane: 17,
        name: "Wait for CI on the SDK bump",
        repo: "web",
        branch: "abc-147-sdk",
        state: "background",
        wait: None,
        minutes_ago: 3,
        prompt: None,
        shape: |t, _| {
            t.background = 1;
            t.monitors = vec!["CI on the SDK bump".into()];
        },
        turns: &[
            ("you", "Bump the SDK and wait for CI.", &[], 22),
            (
                "claude",
                "Pushed the bump; CI is running, I'll watch it.",
                &[],
                3,
            ),
        ],
    },
    Tab {
        pane: 18,
        name: "Write the onboarding guide",
        repo: "docs",
        branch: "abc-162-onboarding",
        state: "background",
        wait: None,
        minutes_ago: 0,
        prompt: None,
        shape: |t, _| looping(t, "review", 2),
        turns: &[(
            "you",
            "Write a guide for a new teammate's first day, from Riley's notes.",
            &[],
            5,
        )],
    },
    Tab {
        pane: 19,
        name: "Flaky login test",
        repo: "web",
        branch: "abc-129-flaky-login",
        state: "idle",
        wait: None,
        minutes_ago: 95,
        prompt: None,
        shape: |t, _| {
            t.pr_state = Some(PrState::Open);
            t.checks = Some(Checks {
                pass: 11,
                fail: 1,
                pending: 0,
            });
        },
        turns: &[
            (
                "you",
                "Why does the login test fail one run in ten?",
                &[],
                130,
            ),
            (
                "claude",
                "It waits for the redirect by a fixed 500 ms. I made it wait for the dashboard's heading instead; 200 runs passed.",
                &[],
                95,
            ),
        ],
    },
    Tab {
        pane: 20,
        name: "Webhook retries",
        repo: "api",
        branch: "abc-121-webhooks",
        state: "idle",
        wait: None,
        minutes_ago: 240,
        prompt: None,
        shape: |t, _| {
            t.pr_state = Some(PrState::Open);
            t.merge = Some(MergeState::Queued);
            t.position = Some(2);
        },
        turns: &[
            ("you", "Retry a failed webhook with backoff.", &[], 300),
            (
                "claude",
                "Done: up to five tries, doubling from 30 seconds.",
                &[],
                240,
            ),
        ],
    },
    Tab {
        pane: 21,
        name: "API reference typos",
        repo: "docs",
        branch: "abc-118-typos",
        state: "idle",
        wait: None,
        minutes_ago: 1500,
        prompt: None,
        shape: plain,
        turns: &[
            (
                "you",
                "Fix the typos Jordan found in the API reference.",
                &[],
                1560,
            ),
            ("claude", "Fixed all eleven.", &[], 1500),
        ],
    },
    Tab {
        pane: 22,
        name: "Trust this folder",
        repo: "web",
        branch: "abc-166-storybook",
        state: "waiting",
        wait: Some("Waiting on your input"),
        minutes_ago: 12,
        prompt: None,
        shape: plain,
        turns: &[],
    },
    Tab {
        pane: 23,
        name: "Run the load test",
        repo: "api",
        branch: "abc-171-load-test",
        state: "background",
        wait: None,
        minutes_ago: 7,
        prompt: None,
        shape: |t, _| {
            t.background = 1;
            t.shells = 1;
        },
        turns: &[
            ("you", "Run the load test against staging.", &[], 9),
            ("tool", "Bash: k6 run load/search.js", &[], 8),
            (
                "claude",
                "The load test runs for ten minutes; I'll read its numbers when it ends.",
                &[],
                7,
            ),
        ],
    },
    Tab {
        pane: 24,
        name: "Fix the importer's lint",
        repo: "api",
        branch: "abc-173-importer-lint",
        state: "background",
        wait: None,
        minutes_ago: 2,
        prompt: None,
        shape: |t, _| looping(t, "ci", 1),
        turns: &[("you", "Get CI green on the importer.", &[], 15)],
    },
    Tab {
        pane: 25,
        name: "Cache headers",
        repo: "web",
        branch: "abc-168-cache-headers",
        state: "background",
        wait: None,
        minutes_ago: 4,
        prompt: None,
        shape: |t, _| {
            t.pr_state = Some(PrState::Open);
            looping(t, "queue", 1);
        },
        turns: &[("you", "/queue-and-clean", &[], 6)],
    },
    Tab {
        pane: 26,
        name: "Morning digest",
        repo: "docs",
        branch: "main",
        state: "background",
        wait: None,
        minutes_ago: 50,
        prompt: None,
        shape: |t, now_ms| {
            t.tab_loop = Some(TabLoop::Scheduled(Scheduled {
                prompt: "/digest".into(),
                at: now_ms + 10 * 60_000,
            }));
        },
        turns: &[("you", "/loop 1h /digest", &[], 120)],
    },
    Tab {
        pane: 27,
        name: "Retry budget for exports",
        repo: "api",
        branch: "abc-170-export-retries",
        state: "idle",
        wait: None,
        minutes_ago: 1,
        prompt: None,
        shape: |t, _| {
            t.landing = Some(Landing {
                holding: true,
                line: 1,
                method: Method::Direct,
            });
        },
        turns: &[("you", "Land it.", &[], 2)],
    },
    Tab {
        pane: 28,
        name: "Billing banner copy",
        repo: "web",
        branch: "abc-164-banner",
        state: "background",
        wait: None,
        minutes_ago: 3,
        prompt: None,
        shape: |t, _| {
            t.merge = Some(MergeState::Merged);
            looping(t, "deploy", 1);
        },
        turns: &[("you", "Test the banner once it deploys.", &[], 20)],
    },
    Tab {
        pane: 29,
        name: "Dark mode for emails",
        repo: "web",
        branch: "abc-158-dark-emails",
        state: "idle",
        wait: None,
        minutes_ago: 70,
        prompt: None,
        shape: |t, _| t.merge = Some(MergeState::Merged),
        turns: &[("claude", "Merged.", &[], 70)],
    },
    Tab {
        pane: 30,
        name: "SSO follow-up",
        repo: "api",
        branch: "abc-175-sso",
        state: "idle",
        wait: None,
        minutes_ago: 30,
        prompt: None,
        shape: |t, now_ms| {
            t.remind_at = Some(now_ms / 1000 + 2 * 3600);
            t.remind_note = Some("after standup".into());
        },
        turns: &[("you", "Ask Sam about the SSO ticket.", &[], 35)],
    },
    Tab {
        pane: LONG,
        name: "Port the billing exports",
        repo: "api",
        branch: "abc-181-billing-exports",
        state: "idle",
        wait: None,
        minutes_ago: 5,
        prompt: None,
        shape: plain,
        // Its turns are `long_conversation`'s.
        turns: &[],
    },
];

/// The pane of the tab with a long conversation, which its channel loads a
/// screen of turns at a time as it is scrolled up (`phone::turns`'s
/// `window`).
const LONG: u32 = 31;

/// How many turns the long conversation has.
const LONG_TURNS: usize = 64;

/// A long made-up conversation, oldest first: `LONG_TURNS` turns, each
/// your ask, a few tool calls and Claude's reply, ending five minutes ago.
fn long_conversation(now_ms: i64) -> Vec<Value> {
    const FILES: &[&str] = &[
        "exports/csv.rs",
        "exports/schedule.rs",
        "billing/invoice.rs",
        "billing/tax.rs",
    ];
    let mut out = Vec::new();
    for turn in 0..LONG_TURNS {
        let ago = ((LONG_TURNS - turn) * 4 + 1) as i64;
        let at = |m: i64| now_ms - m * 60_000;
        let file = FILES[turn % FILES.len()];
        let n = turn + 1;
        out.push(json!({"who": "you", "text": format!("Step {n}: port the next piece of the billing export, starting from {file}."), "images": [], "at": at(ago)}));
        for (i, tool) in [
            format!("Read {file}"),
            format!("Edit {file}"),
            "Bash: cargo test exports".to_string(),
        ]
        .into_iter()
        .enumerate()
        {
            out.push(json!({"who": "tool", "text": tool, "images": [], "at": at(ago) + 20_000 * (i as i64 + 1)}));
        }
        out.push(json!({"who": "claude", "text": format!("Step {n} is done: {file} now writes the export's rows in the new format, and the tests for it pass."), "images": [], "at": at(ago - 2)}));
    }
    out
}

/// A tab nothing but its state places.
fn plain(_: &mut TabModel, _: i64) {}

/// A tab between turns running a loop's work (`review`, `ci`, `queue`,
/// `deploy`) on its round.
fn looping(t: &mut TabModel, kind: &'static str, round: usize) {
    t.background = 1;
    t.tab_loop = Some(TabLoop::Run(LoopRun { kind, round }));
}

fn migration_prompt() -> Value {
    json!({
        "id": "demo-migration",
        "question": "Run the migration on the staging database?",
        "options": [
            {"n": 1, "label": "Yes", "checked": false, "detail": "Runs migrations/0043_orders_v2.sql against staging."},
            {"n": 2, "label": "Yes, and don't ask again for migrations", "checked": false, "detail": null},
            {"n": 3, "label": "No, and tell Claude what to do differently", "checked": false, "detail": null},
        ],
        "multi": false,
        "free": 3,
    })
}

fn checks_prompt() -> Value {
    json!({
        "id": "demo-checks",
        "question": "Which checks should run before the merge?",
        "options": [
            {"n": 1, "label": "Unit tests", "checked": true, "detail": null},
            {"n": 2, "label": "End-to-end tests", "checked": false, "detail": "The browser suite, about 12 minutes."},
            {"n": 3, "label": "Lighthouse", "checked": false, "detail": null},
            {"n": 4, "label": "Type something.", "checked": false, "detail": null},
        ],
        "multi": true,
        "free": 4,
    })
}

/// A question asked by a tab the preview's "Start waiting" button wakes,
/// its id made from `n` so each is a new one.
pub fn new_prompt(n: u64) -> Value {
    json!({
        "id": format!("demo-asked-{n}"),
        "question": "Open a PR for this branch?",
        "options": [
            {"n": 1, "label": "Yes", "checked": false, "detail": null},
            {"n": 2, "label": "Not yet", "checked": false, "detail": "Leaves the branch as it is, unpushed."},
            {"n": 3, "label": "No, and tell Claude what to do differently", "checked": false, "detail": null},
        ],
        "multi": false,
        "free": 3,
    })
}

/// What a made-up Claude says when a turn it was working on ends.
pub const REPLY: &str =
    "Done. I made the change, ran the tests (all passing) and left the branch ready for review.";

/// The made-up tabs as `watch` lists them, `now_ms` being now, in the
/// sidebar's groups and order.
pub fn tabs(now_ms: i64) -> Vec<Value> {
    let mut tabs: Vec<Value> = TABS
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let last_you = t.turns.iter().rev().find(|(who, ..)| *who == "you");
            let last_claude = t.turns.iter().rev().find(|(who, ..)| *who == "claude");
            json!({
                "tab": format!("@{}", t.pane),
                "index": i + 1,
                "name": t.name,
                "repo": t.repo,
                "branch": t.branch,
                "state": t.state,
                "pinned": false,
                "since": (now_ms - t.minutes_ago * 60_000) / 1000,
                "pane": format!("%{}", t.pane),
                "wait": t.wait,
                "transcript": null,
                "turn": last_you.map(|(_, text, _, ago)| json!({
                    "prompt": text,
                    "reply": last_claude.map(|(_, reply, ..)| *reply),
                    "at": now_ms - ago * 60_000,
                })),
                "doing": doing_of(t.pane, now_ms),
                "prompt": t.prompt.map(|p| p()),
                "tab_views": tab_rows(t.pane),
            })
        })
        .collect();
    restage(&mut tabs, now_ms);
    tabs
}

/// The made-up PR of a tab with one, as the lookups would hold it, and
/// the PR view's copy with its description and checks.
fn made_up_pr(pane: u32) -> Option<(crate::facts::PrFacts, Value)> {
    let (number, title, fail, pending, review) = match pane {
        11 => (
            142,
            "Move the orders table to the new schema",
            1,
            0,
            "CHANGES_REQUESTED",
        ),
        13 => (131, "Rate limit by token", 0, 1, "REVIEW_REQUIRED"),
        _ => return None,
    };
    let facts = crate::facts::PrFacts {
        number,
        title: title.into(),
        head: String::new(),
        draft: false,
        repo: "acme/api".into(),
        url: format!("https://github.com/acme/api/pull/{number}"),
        state: PrState::Open,
        merge: None,
        checks: Some(Checks {
            pass: 4,
            fail,
            pending,
        }),
        review_decision: Some(review.into()),
        additions: 120,
        deletions: 40,
    };
    let check = |name: &str, status: &str, conclusion: &str| json!({ "__typename": "CheckRun", "name": name, "status": status, "conclusion": conclusion });
    let mut checks = vec![
        check("Formatting", "COMPLETED", "SUCCESS"),
        check("Lint", "COMPLETED", "SUCCESS"),
        check("Build", "COMPLETED", "SUCCESS"),
        check("Docs", "COMPLETED", "SUCCESS"),
    ];
    if fail > 0 {
        checks.push(check("Tests (Linux)", "COMPLETED", "FAILURE"));
    }
    if pending > 0 {
        checks.push(check("Tests (macOS)", "IN_PROGRESS", ""));
    }
    let copy = json!({
        "number": number,
        "body": format!("{title}, in batches, so a large table never holds a lock for long.\n\n## Before this PR\n\nThe table was copied in one statement.\n\nSee the [runbook](https://example.com/runbook)."),
        "last": { "nodes": [{ "commit": { "statusCheckRollup": { "contexts": { "nodes": checks } } } }] },
    });
    Some((facts, copy))
}

/// The made-up ticket of a tab with one, as the ticket view last read it.
fn made_up_ticket(pane: u32) -> Option<(crate::views::ticket::Ticket, Value)> {
    let (id, title) = match pane {
        11 => ("ABC-142", "Orders table migration"),
        12 => ("ABC-150", "Release checklist for 2.4"),
        _ => return None,
    };
    let issue = json!({
        "identifier": id,
        "title": title,
        "url": format!("https://linear.app/acme/issue/{id}"),
        "description": format!("## Today\n\n{title} is done by hand.\n\n## What to build\n\n- Script it\n- Keep the old path working"),
        "state": { "name": "In Progress", "type": "started" },
    });
    Some((crate::views::ticket::Ticket::Linear(id.into()), issue))
}

/// The tool call a made-up working tab is on, as the server sends it
/// (`phone::serve`'s `doing_json`): its tool, its argument's first line and
/// when it started, in seconds.
fn doing_of(pane: u32, now_ms: i64) -> Value {
    match pane {
        16 => json!({
            "tool": "Bash",
            "arg": "cargo test search::page -- --nocapture",
            "since": now_ms / 1000 - 40,
        }),
        _ => Value::Null,
    }
}

/// The made-up note of a tab with one, its file's name and its text.
fn made_up_note(pane: u32) -> Option<(&'static str, &'static str)> {
    match pane {
        11 | 14 => Some((
            "plans/Plan - Orders Migration.md",
            "# Plan - Orders Migration\n\n## Steps\n\n- [x] Write the migration\n- [x] Batch the copy\n- [ ] Run it on staging\n- [ ] Swap the tables\n\n## Links\n\n- [the runbook](https://example.com/runbook)\n",
        )),
        _ => None,
    }
}

/// What of its PR, ticket and note a made-up tab has, as the server's
/// snapshot lists them (`phone::tab_views`).
fn tab_rows(pane: u32) -> Value {
    use crate::phone::tab_views;
    let mut rows = Vec::new();
    if let Some((facts, _)) = made_up_pr(pane) {
        rows.push(tab_views::pr_row(facts.number, Some(&facts)));
    }
    if let Some((ticket, issue)) = made_up_ticket(pane) {
        rows.push(tab_views::ticket_row(&ticket.short(), Some(&issue)));
    }
    if let Some((note, _)) = made_up_note(pane) {
        rows.push(tab_views::note_row(note));
    }
    Value::Array(rows)
}

/// A made-up tab's PR, ticket or note as `phone-tab` answers it, by its
/// pane's number and the kind; none for one it hasn't.
pub fn tab_view(pane: &str, kind: &str) -> Option<Value> {
    use crate::phone::tab_views;
    let pane: u32 = pane.parse().ok()?;
    match kind {
        "pr" => made_up_pr(pane)
            .map(|(f, copy)| tab_views::pr_json(f.number, Some(&f), Some(&copy), None)),
        "ticket" => made_up_ticket(pane).map(|(t, issue)| tab_views::ticket_json(&t, Some(&issue))),
        "note" => made_up_note(pane).map(|(note, text)| tab_views::note_json(note, text)),
        _ => None,
    }
}

/// The made-up tabs, as the preview has changed them, placed, marked and
/// worded again as the sidebar would, and put in its order: each made a
/// sidebar tab from its state, since, wait, pin and reminder, and, while
/// its state is the one it started in, what put it in its group then.
pub fn restage(tabs: &mut Vec<Value>, now_ms: i64) {
    let r = Rules {
        now_ms,
        reminders: true,
        build_failing: true,
        panel: false,
        scroll_bars: false,
    };
    let models: Vec<TabModel> = tabs.iter().map(|t| model_of(t, now_ms)).collect();
    let placed: Vec<Value> = crate::phone::drawer::staged(&models, &r)
        .into_iter()
        .filter_map(|(stage, m)| {
            let at = models.iter().position(|x| std::ptr::eq(x, m))?;
            let mut tab = tabs[at].clone();
            // A working tab with no question takes Stop, as the server
            // tells the phone (`stoppable`).
            tab["stoppable"] = json!(tab["state"] == "working" && tab["prompt"].is_null());
            tab.as_object_mut()?
                .extend(crate::phone::drawer::fields(m, stage, &r));
            Some(tab)
        })
        .collect();
    *tabs = placed;
}

/// A made-up tab as the sidebar models it.
fn model_of(tab: &Value, now_ms: i64) -> TabModel {
    let pane = tab["pane"].as_str().unwrap_or("").trim_start_matches('%');
    let made = TABS.iter().find(|t| t.pane.to_string() == pane);
    let index = tab["index"].as_u64().unwrap_or(0) as usize;
    let state = tab["state"].as_str().unwrap_or("");
    let mut m = TabModel::new(
        TabId(pane.parse().unwrap_or(0)),
        index,
        tab["name"].as_str().unwrap_or(""),
    );
    m.state = match state {
        "waiting" => TabState::Waiting,
        "done" => TabState::Done,
        "working" => TabState::Working,
        "background" => TabState::Background,
        _ => TabState::Idle,
    };
    m.claude = Some(SessionRef::default());
    m.since = tab["since"].as_i64();
    m.wait = tab["wait"].as_str().map(str::to_string);
    if let Some(made) = made.filter(|t| t.state == state) {
        (made.shape)(&mut m, now_ms);
    }
    m.pinned = tab["pinned"] == true;
    // A reminder the preview set or cleared over the one it started with.
    if let Some(remind) = tab.get("remind") {
        m.remind_at = remind["at"].as_i64();
        m.remind_note = remind["note"].as_str().map(str::to_string);
    }
    m
}

/// Each made-up tab's turns as `phone::turns` reads them, by its pane
/// (`%11`).
pub fn turns(now_ms: i64) -> Vec<(String, Vec<Value>)> {
    TABS.iter()
        .map(|t| {
            if t.pane == LONG {
                return (format!("%{}", t.pane), long_conversation(now_ms));
            }
            let messages = t
                .turns
                .iter()
                .map(|(who, text, images, ago)| {
                    let at = now_ms - ago * 60_000;
                    if *who == "look" {
                        // A look of two is a before and after.
                        let look = crate::phone::looks::Look {
                            at,
                            images: images.iter().map(|i| i.to_string()).collect(),
                            before_after: images.len() == 2,
                        };
                        return json!(crate::phone::looks::message(&look));
                    }
                    json!({"who": who, "text": text, "images": images, "at": at})
                })
                .collect();
            (format!("%{}", t.pane), messages)
        })
        .collect()
}

/// A made-up view's item: its row and its menu's options, each a label
/// and what the phone does with it (`phone::views::Kind`).
type MadeUp = (Item, Vec<(&'static str, Kind)>);

/// Mission control's views as the phone's panel lists them (boards 185 to
/// 187), made up: Landing's train, queue and what landed, Autopilot's tabs,
/// the PR board's columns, CI's runs, the day's notifications and Usage, each with
/// options as the server offers them, in the shape `phone::mission` sends.
pub fn views(now_ms: i64) -> Vec<Value> {
    let ago = |m: i64| now_ms - m * 60_000;
    let pr = |n: u32| format!("https://github.com/acme/web/pull/{n}");
    let go = |pane: u32| Kind::Go(format!("%{pane}"));
    let run = || Kind::Run(None);
    let asked = |q: &str| Kind::Run(Some(q.to_string()));
    let pr_options = |n: u32, pane: Option<u32>| -> Vec<(&'static str, Kind)> {
        let mut out = Vec::new();
        if let Some(p) = pane {
            out.push(("Go to tab, where it is open", go(p)));
        }
        out.push((
            "Work on it in a new tab (wtpr)",
            asked("Work on it in a new tab (wtpr)?"),
        ));
        out.push(("Open it in the browser", Kind::Open(pr(n))));
        out.push(("Copy its link", Kind::Copy(pr(n))));
        out
    };
    let landing: Vec<MadeUp> = vec![
        (header("train", "Train 14 · testing", 2), vec![]),
        (
            Item::new("q88", "#88 benchmark harness")
                .detail("tab 5 · 6 checks running")
                .state(State::Active)
                .time(ago(4)),
            vec![
                ("Open the PR", Kind::Open(pr(88))),
                ("Copy its link", Kind::Copy(pr(88))),
            ],
        ),
        (
            Item::new("q91", "#91 fork parity fixture")
                .detail("tab 7 · 6 checks running")
                .state(State::Active)
                .time(ago(4)),
            vec![
                ("Open the PR", Kind::Open(pr(91))),
                ("Copy its link", Kind::Copy(pr(91))),
            ],
        ),
        (header("queue", "In the queue", 2), vec![]),
        (
            Item::new("q93", "#93 stable note IDs")
                .detail("tab 2 · next train")
                .state(State::Active),
            vec![
                ("Go to tab 2", go(12)),
                ("Move it down in the merge queue", run()),
                ("Open the PR", Kind::Open(pr(93))),
                ("Copy its link", Kind::Copy(pr(93))),
                (
                    "Take it out of the merge queue",
                    asked("Take #93 out of the merge queue?"),
                ),
            ],
        ),
        (
            Item::new("q94", "#94 full titles")
                .detail("tab 4 · after it")
                .state(State::Active),
            vec![
                ("Move it up in the merge queue", run()),
                (
                    "Take it out of the merge queue",
                    asked("Take #94 out of the merge queue?"),
                ),
            ],
        ),
        (header("landed", "Landed today", 1), vec![]),
        (
            Item::new("q86", "#86 redraw timing")
                .detail("train 13 · merged 9:12")
                .time(ago(31)),
            vec![("Open the PR", Kind::Open(pr(86)))],
        ),
    ];
    let autopilot: Vec<MadeUp> = vec![
        (
            Item::new("a1", "review PR 41")
                .detail("tab 6 · asks: which fixture?")
                .state(State::Attention)
                .group("Waiting on you"),
            vec![
                ("Go to tab 6", go(16)),
                ("Skip its item: review PR 41", run()),
            ],
        ),
        (
            Item::new("a2", "planning")
                .detail("tab 1 · Monitor: fork parity")
                .state(State::Active)
                .group("Working"),
            vec![("Go to tab 1", go(11)), ("Skip its item: planning", run())],
        ),
    ];
    let prs: Vec<MadeUp> = vec![
        (
            Item::new("p41", "#41 review PR 41")
                .detail("tab 6 · 2 comments")
                .state(State::Attention)
                .group("Changes asked")
                .time(ago(60)),
            pr_options(41, Some(16)),
        ),
        (
            Item::new("p90", "#90 split the parser")
                .detail("yours · 1 reviewer")
                .group("In review")
                .time(ago(180)),
            pr_options(90, None),
        ),
        (
            Item::new("p87", "#87 theme the picker")
                .detail("asks your review")
                .group("In review")
                .time(ago(1440)),
            pr_options(87, None),
        ),
        (
            Item::new("p92", "#92 drop the old hook")
                .detail("tab 8 · checks passed")
                .group("Approved")
                .time(ago(20)),
            pr_options(92, Some(18)),
        ),
        (
            Item::new("p93", "#93 stable note IDs")
                .detail("tab 2 · 1st")
                .state(State::Active)
                .group("In merge queue"),
            pr_options(93, Some(12)),
        ),
    ];
    let ci_item = |id: &str, title: &str, detail: &str, state: &str, minutes: i64| {
        let tone = if state == "failing" {
            State::Attention
        } else {
            State::Idle
        };
        Item::new(id, title)
            .detail(detail)
            .state(tone)
            .time(ago(minutes))
            .data(json!({"ci": {"state": state}}))
    };
    let ci: Vec<MadeUp> = vec![
        (header("failing", "Failing", 1), vec![]),
        (
            ci_item(
                "c90",
                "#90 Clippy (Linux x86_64)",
                "split the parser · 1 of 6",
                "failing",
                3,
            ),
            vec![
                (
                    "Start a CI loop on it in tab 11",
                    asked("Start a CI loop on it in tab 11?"),
                ),
                ("Go to tab 11, where it is open", go(11)),
                (
                    "Open the failing check in the browser",
                    Kind::Open(pr(90) + "/checks"),
                ),
                ("Open the PR in the browser", Kind::Open(pr(90))),
                ("Copy its link", Kind::Copy(pr(90))),
            ],
        ),
        (header("running", "Running", 1), vec![]),
        (
            ci_item("c-train", "Train 14", "#88, #91 · 6 checks", "running", 4),
            vec![],
        ),
        (header("passing", "Passing", 2), vec![]),
        (
            ci_item("c92", "#92 drop the old hook", "6 of 6", "passing", 20),
            vec![("Open the PR in the browser", Kind::Open(pr(92)))],
        ),
        (
            ci_item("c-main", "main", "after train 13", "passing", 31),
            vec![],
        ),
    ];
    let dismiss = || ("Dismiss it", Kind::Run(None));
    let notifications: Vec<MadeUp> = vec![
        (header("today", "Today", 5), vec![]),
        (
            Item::new("n1", "CI failed on #90")
                .detail("Clippy (Linux x86_64)")
                .state(State::Attention)
                .time(ago(3)),
            vec![
                ("Open the PR in the browser", Kind::Open(pr(90))),
                dismiss(),
            ],
        ),
        (
            Item::new("n2", "Tab 6 asks")
                .detail("which fixture?")
                .state(State::Attention)
                .time(ago(9)),
            vec![("Go to tab 6", go(16)), dismiss()],
        ),
        (
            Item::new("n3", "#86 landed")
                .detail("train 13")
                .time(ago(31)),
            vec![dismiss()],
        ),
        (
            Item::new("n4", "Tab 8: handoff")
                .detail("your reminder came due")
                .state(State::Attention)
                .time(ago(60)),
            vec![("Go to tab 8", go(18)), dismiss()],
        ),
        (
            Item::new("n5", "Review asked on #87")
                .detail("theme the picker")
                .time(ago(1440)),
            vec![("Open it in the browser", Kind::Open(pr(87))), dismiss()],
        ),
    ];
    let made = |id: ViewId, title: &str, rows: Vec<MadeUp>, acts: Vec<(&str, Kind)>| {
        let view = View {
            id,
            title: title.to_string(),
            data: ViewData {
                items: rows.iter().map(|(i, _)| i.clone()).collect(),
                ..ViewData::default()
            },
            loaded: true,
        };
        let options = |item: &Item| -> Vec<Value> {
            rows.iter()
                .find(|(i, _)| i.id == item.id)
                .map(|(_, o)| o.iter().map(|(l, k)| views::option_json(l, k)).collect())
                .unwrap_or_default()
        };
        let acts = acts.iter().map(|(l, k)| views::option_json(l, k)).collect();
        views::view_json(&view, &options, acts)
    };
    vec![
        made(ViewId::Landing, "Landing", landing, vec![]),
        made(ViewId::Autopilot, "Autopilot", autopilot, vec![]),
        made(
            ViewId::Prs,
            "PRs",
            prs,
            vec![
                ("Read every PR again now", Kind::Run(None)),
                (
                    "Open the PRs waiting on your review on GitHub",
                    Kind::Open("https://github.com/pulls/review-requested".into()),
                ),
            ],
        ),
        made(
            ViewId::Ci,
            "CI",
            ci,
            vec![
                ("Read CI again now", Kind::Run(None)),
                (
                    "Re-run main's failed jobs…",
                    Kind::Run(Some(
                        "Run the failed jobs of 1 default branch run again on GitHub?".into(),
                    )),
                ),
            ],
        ),
        made(
            ViewId::Notifications,
            "Notifications",
            notifications,
            vec![],
        ),
        usage(now_ms),
    ]
}

/// The Usage view, made up: the 5-hour limit filling past itself at its
/// pace, the week's comfortably, today's and the week's spend, and the
/// day's top tabs by spend, one of them closed since.
fn usage(now_ms: i64) -> Value {
    use crate::usage::charts::{self, Limit, Model, SpanSpend, TabSpend};
    let now = now_ms.div_euclid(1000);
    let tab = |pane: Option<u32>, index: usize, name: &str, spent: f64| TabSpend {
        key: format!("tab:{index}"),
        tab: pane,
        index,
        name: name.into(),
        spent,
        ..TabSpend::default()
    };
    let limit = |key: &str, used: f64, left: i64, pace: Option<f64>| Limit {
        key: key.into(),
        name: crate::usage::sessions::limit_name(key),
        used,
        resets_at: now + left,
        pace,
        detail: None,
    };
    let model = Model {
        today: SpanSpend {
            total: 14.2,
            tabs: vec![
                tab(Some(15), 5, "Checkout page layout", 5.1),
                tab(Some(11), 1, "Migrate the orders table", 3.4),
                tab(None, 27, "Reword the pricing page", 2.6),
                tab(Some(13), 3, "Rate limit by token", 1.9),
                tab(Some(19), 9, "Flaky login test", 1.2),
            ],
            ..SpanSpend::default()
        },
        week: SpanSpend {
            total: 86.75,
            ..SpanSpend::default()
        },
        limits: vec![
            limit("five_hour", 62.0, 2 * 3600 + 600, Some(104.0)),
            limit("seven_day", 41.0, 3 * 86400, Some(57.0)),
        ],
        ..Model::default()
    };
    let view = View {
        id: ViewId::Usage,
        title: "Usage".into(),
        data: ViewData {
            items: charts::items(vec![Item::new("spent", "$14.20 today")], &model),
            ..ViewData::default()
        },
        loaded: true,
    };
    // The made-up tabs' Claude panes are their own ids.
    let open = |t: TabId| Some(format!("%{}", t.0));
    views::usage_json(&view, now, &open)
}

/// The images the made-up turns name: drawings of a phone's screen, in SVG.
pub fn image(name: &str) -> Option<String> {
    let (title, line) = match name {
        "demo-checkout.svg" => ("Checkout", "Total $48.20 wraps"),
        "demo-total.svg" => ("Order summary", "Pay now ⟶"),
        "demo-before.svg" => ("Order summary", "Total $48.20 wraps"),
        "demo-after.svg" => ("Order summary", "Total $48.20 · Pay"),
        _ => return None,
    };
    Some(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="600" viewBox="0 0 300 600"><rect width="300" height="600" rx="28" fill="#1d2030"/><rect x="20" y="40" width="260" height="40" rx="8" fill="#2c3147"/><text x="34" y="66" fill="#e6e6f0" font-family="sans-serif" font-size="18">{title}</text><rect x="20" y="110" width="260" height="300" rx="8" fill="#262a3d"/><rect x="20" y="440" width="260" height="56" rx="12" fill="#5f9fd7"/><text x="40" y="474" fill="#11121a" font-family="sans-serif" font-size="18">{line}</text></svg>"##
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every group the sidebar has, and each case the issue names.
    #[test]
    fn the_tabs_cover_every_state_and_case() {
        let tabs = tabs(1_000_000_000);
        for state in ["waiting", "done", "working", "background", "idle"] {
            assert!(tabs.iter().any(|t| t["state"] == state), "{state}");
        }
        for stage in &crate::sidebar::model::SETTABLE[..14] {
            assert!(
                tabs.iter().any(|t| t["stage"] == stage.key()),
                "{}",
                stage.key()
            );
        }
        assert!(tabs.iter().any(|t| t["words"] == "waiting on 1 shell"));
        assert!(tabs.iter().any(|t| t["words"] == "in merge queue at 2"));
        assert!(tabs.iter().any(|t| t["prompt"]["multi"] == false));
        assert!(tabs.iter().any(|t| t["prompt"]["multi"] == true));
        assert!(
            turns(1_000_000_000)
                .iter()
                .flat_map(|(_, m)| m)
                .any(|m| m["who"] == "look" && m["titles"] == json!(["Before", "After"]))
        );
        let turns = turns(1_000_000_000);
        assert!(
            turns
                .iter()
                .flat_map(|(_, m)| m)
                .any(|m| !m["images"].as_array().unwrap().is_empty())
        );
        for (_, messages) in &turns {
            for m in messages {
                for name in m["images"].as_array().unwrap() {
                    assert!(image(name.as_str().unwrap()).is_some(), "{name}");
                }
            }
        }
        // Needs you first.
        let first_not = tabs.iter().position(|t| t["needs"] == false).unwrap();
        assert!(tabs[first_not..].iter().all(|t| t["needs"] == false));
    }
}
