//! The phone app's made-up tabs, for `switchboard phone preview`
//! (`phone::preview`): a dozen Claude tabs in every state the drawer
//! groups, as the server's `watch` lists them (`phone::serve`'s
//! `phone_snapshot`), and a few turns each, as `phone::turns` reads them
//! from a transcript. One waits on a question with choices, one on a
//! question that takes several, one finished with a long reply in
//! Markdown, one finished on a failure, one with images you sent; the rest
//! work, wait in the background or sit idle. Every name is made up, in the
//! demo's style (`acme/` repositories, people by first name).

use serde_json::{Value, json};

/// A made-up tab: its pane's number, name, repository, branch, state and
/// what it is doing, and its turns, oldest first, each `(who, text,
/// images, minutes ago)`.
struct Tab {
    pane: u32,
    name: &'static str,
    repo: &'static str,
    branch: &'static str,
    state: &'static str,
    wait: Option<&'static str>,
    minutes_ago: i64,
    prompt: Option<fn() -> Value>,
    turns: &'static [(&'static str, &'static str, &'static [&'static str], i64)],
}

const LONG_REPLY: &str = "## What changed\n\nThe rate limiter now keys on the **API token** rather than the client's address, so two services behind one proxy no longer share a budget.\n\n1. `limits.rs` reads the token from the `Authorization` header.\n2. A request with no token falls back to the address, as before.\n3. The budget is kept in Redis under `limit:<token>`, expiring after a minute.\n\n## Tests\n\n- `limits::a_token_has_its_own_budget`\n- `limits::no_token_falls_back_to_the_address`\n- `limits::the_budget_resets_after_a_minute`\n\nAll 214 tests pass. The load test held 1,200 requests a second with the p99 at 41 ms, against 38 ms before.\n\n```rust\nlet key = token.map_or_else(|| format!(\"limit:{addr}\"), |t| format!(\"limit:{t}\"));\n```\n\n## Left for you\n\nThe dashboards still group by address: Sam owns them, so I left a note in the PR rather than changing them.";

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
        turns: &[
            (
                "you",
                "Make the rate limiter key on the API token rather than the address.",
                &[],
                52,
            ),
            ("tool", "Edit src/limits.rs", &[], 40),
            ("tool", "Bash: cargo test", &[], 12),
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
        turns: &[
            (
                "you",
                "The total wraps under the button on a small phone, see these.",
                &["demo-checkout.svg", "demo-total.svg"],
                40,
            ),
            ("tool", "Edit src/checkout/Summary.tsx", &[], 33),
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
        wait: Some("Bash: cargo test search"),
        minutes_ago: 1,
        prompt: None,
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
        wait: Some("Waiting on CI"),
        minutes_ago: 3,
        prompt: None,
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
        state: "working",
        wait: None,
        minutes_ago: 0,
        prompt: None,
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
        turns: &[],
    },
];

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

/// The made-up tabs as `watch` lists them, `now_ms` being now.
pub fn tabs(now_ms: i64) -> Vec<Value> {
    let mut tabs: Vec<Value> = TABS
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let at = now_ms - t.minutes_ago * 60_000;
            let last_you = t.turns.iter().rev().find(|(who, ..)| *who == "you");
            let last_claude = t.turns.iter().rev().find(|(who, ..)| *who == "claude");
            json!({
                "tab": format!("@{}", t.pane),
                "index": i + 1,
                "name": t.name,
                "repo": t.repo,
                "branch": t.branch,
                "state": t.state,
                "needs": t.state == "waiting" || t.state == "done",
                "since": at / 1000,
                "pane": format!("%{}", t.pane),
                "wait": t.wait,
                "transcript": null,
                "turn": last_you.map(|(_, text, _, ago)| json!({
                    "prompt": text,
                    "reply": last_claude.map(|(_, reply, ..)| *reply),
                    "at": now_ms - ago * 60_000,
                })),
                "prompt": t.prompt.map(|p| p()),
            })
        })
        .collect();
    // Needs you first, as the server sorts them.
    tabs.sort_by_key(|t| !t["needs"].as_bool().unwrap_or(false));
    tabs
}

/// Each made-up tab's turns as `phone::turns` reads them, by its pane
/// (`%11`).
pub fn turns(now_ms: i64) -> Vec<(String, Vec<Value>)> {
    TABS.iter()
        .map(|t| {
            let messages = t
                .turns
                .iter()
                .map(|(who, text, images, ago)| {
                    json!({"who": who, "text": text, "images": images, "at": now_ms - ago * 60_000})
                })
                .collect();
            (format!("%{}", t.pane), messages)
        })
        .collect()
}

/// The images the made-up turns name: drawings of a phone's screen, in SVG.
pub fn image(name: &str) -> Option<String> {
    let (title, line) = match name {
        "demo-checkout.svg" => ("Checkout", "Total $48.20 wraps"),
        "demo-total.svg" => ("Order summary", "Pay now ⟶"),
        _ => return None,
    };
    Some(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="600" viewBox="0 0 300 600"><rect width="300" height="600" rx="28" fill="#1d2030"/><rect x="20" y="40" width="260" height="40" rx="8" fill="#2c3147"/><text x="34" y="66" fill="#e6e6f0" font-family="sans-serif" font-size="18">{title}</text><rect x="20" y="110" width="260" height="300" rx="8" fill="#262a3d"/><rect x="20" y="440" width="260" height="56" rx="12" fill="#5f9fd7"/><text x="40" y="474" fill="#11121a" font-family="sans-serif" font-size="18">{line}</text></svg>"##
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state the drawer groups, and each case the issue names.
    #[test]
    fn the_tabs_cover_every_state_and_case() {
        let tabs = tabs(1_000_000_000);
        assert_eq!(tabs.len(), 12);
        for state in ["waiting", "done", "working", "background", "idle"] {
            assert!(tabs.iter().any(|t| t["state"] == state), "{state}");
        }
        assert!(tabs.iter().any(|t| t["prompt"]["multi"] == false));
        assert!(tabs.iter().any(|t| t["prompt"]["multi"] == true));
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
