//! Linear, over its GraphQL API: the API key, a query, a ticket by its
//! identifier as `tmux/ticket-title.sh` fetched its title, and the ticket a
//! branch names, as `tmux/path-facts.sh` matched it.
//!
//! The key is the one the ticket scripts used, first that answers:
//! `LINEAR_API_KEY`, the stored secret `linear-api-key` (the Keychain's, as
//! before), else `linearApiKey` in `~/.config/switchboard/linear.json`, the
//! file `worktree-from-ticket` kept it in. What the secret or the file gave
//! is kept ten minutes and forgotten once Linear refuses it. It is sent as
//! `Authorization: <key>`, as Linear takes a personal API key.
//!
//! Every call refuses while the `linear` setting is off, and is counted in
//! the request audit under Linear, as "GraphQL".

use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Credential, Remembered, ensure_on, env_value, stored_secret};
use crate::net::{self, Audit, ErrorKind, Response, ServiceError};
use crate::{paths, util};

const SERVICE: &str = "Linear";

/// The GraphQL endpoint (`SWITCHBOARD_LINEAR_API` moves it).
pub fn api_url() -> String {
    net::base_url("SWITCHBOARD_LINEAR_API", "https://api.linear.app/graphql")
}

// ── The key ─────────────────────────────────────────────────────────────────

static REMEMBERED: Remembered = Remembered::new();

/// The API key to send, as the module's comment says where it is looked for.
pub async fn api_key() -> Result<String> {
    Ok(credential().await?.value)
}

async fn credential() -> Result<Credential> {
    if let Some(value) = env_value("LINEAR_API_KEY") {
        return Ok(Credential {
            value,
            from: "LINEAR_API_KEY".to_string(),
        });
    }
    if let Some(known) = REMEMBERED.get() {
        return Ok(known);
    }
    let found = match stored_secret("linear-api-key").await {
        Some(value) => Some(Credential {
            value,
            from: "the stored secret linear-api-key".to_string(),
        }),
        None => util::read_json_object(paths::config_dir().join("linear.json"))
            .get("linearApiKey")
            .and_then(Value::as_str)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .map(|value| Credential {
                value,
                from: "linear.json".to_string(),
            }),
    };
    match found {
        Some(credential) => {
            REMEMBERED.set(&credential);
            Ok(credential)
        }
        None => Err(ServiceError::new(
            ErrorKind::NoCredentials,
            SERVICE,
            "No Linear API key: make one in Linear (Settings → Security & access → Personal API \
             keys), then store it with `switchboard platform secret-set linear-api-key` (it reads \
             the key from stdin), or set LINEAR_API_KEY.",
        )
        .into()),
    }
}

/// Forgets the key the secret or the file gave, so the next request looks
/// again.
pub fn forget_key() {
    REMEMBERED.forget();
}

// ── Requests ────────────────────────────────────────────────────────────────

/// A GraphQL query's `data`. Linear's errors in the answer are errors here,
/// with its message: a ticket it has no such one of is `ErrorKind::NotFound`,
/// a key it refuses `Unauthorized`, its rate limit spent `RateLimited`.
/// `source` says what asked, for the request audit.
pub async fn graphql(query: &str, variables: Value, source: &str) -> Result<Value> {
    ensure_on("linear", SERVICE)?;
    let credential = credential().await?;
    let body = json!({ "query": query, "variables": variables });
    let headers = [("Authorization", credential.value.as_str())];
    let audit = Audit {
        service: SERVICE,
        protocol: "GraphQL",
        source,
    };
    let response = net::post_json(&api_url(), &headers, &body, audit).await?;
    // Linear says what went wrong in `errors` whatever the status (often a
    // 400), so they are read first, and the status only when there are none.
    if let Some(errors) = response.body.get("errors").and_then(Value::as_array)
        && !errors.is_empty()
    {
        return Err(linear_error(errors, &response, &credential).into());
    }
    if response.status == 401 {
        return Err(refused(&credential, "401").into());
    }
    if !response.ok() {
        return Err(ServiceError::new(
            ErrorKind::Failed,
            SERVICE,
            format!("Linear answered {}", response.status),
        )
        .into());
    }
    match response.body.get("data") {
        Some(data) if !data.is_null() => Ok(data.clone()),
        _ => Err(
            ServiceError::new(ErrorKind::Failed, SERVICE, "Linear's answer had no data.").into(),
        ),
    }
}

fn refused(credential: &Credential, why: &str) -> ServiceError {
    forget_key();
    ServiceError::new(
        ErrorKind::Unauthorized,
        SERVICE,
        format!(
            "Linear refused the API key from {} ({why}): make a new one and store it with \
             `switchboard platform secret-set linear-api-key`.",
            credential.from
        ),
    )
}

fn linear_error(errors: &[Value], response: &Response, credential: &Credential) -> ServiceError {
    // Linear's message for a person, where it gives one, else its own.
    let words = |e: &Value| {
        e.pointer("/extensions/userPresentableMessage")
            .and_then(Value::as_str)
            .or_else(|| e.get("message").and_then(Value::as_str))
            .map(str::to_string)
    };
    let messages: Vec<String> = errors.iter().filter_map(words).collect();
    let message = if messages.is_empty() {
        "an error it didn't explain".to_string()
    } else {
        messages.join("; ")
    };
    let code_of = |e: &Value| {
        e.pointer("/extensions/code")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let codes: Vec<String> = errors.iter().map(code_of).collect();
    let has = |code: &str| codes.iter().any(|c| c == code);
    if has("AUTHENTICATION_ERROR") || response.status == 401 {
        return refused(credential, &message);
    }
    if has("RATELIMITED") {
        // The reset is in milliseconds since the epoch.
        let reset_at = response
            .header("x-ratelimit-requests-reset")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .map(|ms| ms / 1000);
        return ServiceError::new(
            ErrorKind::RateLimited { reset_at },
            SERVICE,
            format!("Linear's rate limit is spent: {message}"),
        );
    }
    // "Entity not found" is an INPUT_ERROR, as is a malformed argument; only
    // the first is something that isn't there.
    let missing = errors.iter().any(|e| {
        code_of(e) == "INPUT_ERROR"
            && e.get("message")
                .and_then(Value::as_str)
                .is_some_and(|m| m.to_lowercase().contains("not found"))
    });
    let kind = if missing {
        ErrorKind::NotFound
    } else {
        ErrorKind::Failed
    };
    ServiceError::new(kind, SERVICE, format!("Linear: {message}"))
}

// ── Tickets ─────────────────────────────────────────────────────────────────

/// A ticket, as the tab names and views need it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    pub id: String,
    /// `ENG-123`.
    pub identifier: String,
    pub title: String,
    pub url: String,
    /// Its state's name ("In Progress").
    pub state: String,
    pub assignee: Option<Person>,
}

/// Someone in Linear: their id, and the name Linear shows for them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    pub id: String,
    pub name: String,
}

const TICKET: &str = "query($id: String!) { issue(id: $id) { id identifier title url \
                      state { name } assignee { id name displayName } } }";

/// The ticket `identifier` names (`ENG-123`), or `None` when Linear has no
/// such ticket, which is the usual answer for a branch that only looks like
/// it names one.
pub async fn ticket(identifier: &str, source: &str) -> Result<Option<Ticket>> {
    let data = match graphql(TICKET, json!({ "id": identifier }), source).await {
        Ok(data) => data,
        Err(e) if net::kind_of(&e) == Some(&ErrorKind::NotFound) => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(ticket_from(data.get("issue").unwrap_or(&Value::Null)))
}

// A ticket from the `issue` Linear gave, or `None` when it gave none, or one
// without the fields every ticket has.
fn ticket_from(issue: &Value) -> Option<Ticket> {
    let text = |path: &str| {
        issue
            .pointer(path)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let assignee = issue
        .get("assignee")
        .filter(|a| a.is_object())
        .and_then(|a| {
            let id = a.get("id").and_then(Value::as_str)?.to_string();
            let name = a
                .get("displayName")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
                .or_else(|| a.get("name").and_then(Value::as_str))
                .unwrap_or("")
                .to_string();
            Some(Person { id, name })
        });
    Some(Ticket {
        id: text("/id")?,
        identifier: text("/identifier")?,
        title: text("/title")?,
        url: text("/url").unwrap_or_default(),
        state: text("/state/name").unwrap_or_default(),
        assignee,
    })
}

// Any team key, as path-facts.sh and linear-ticket.sh matched it: letters
// and digits starting with a letter, a dash, a number. A branch that only
// looks like a ticket is harmless: Linear has no such ticket, and the tab
// keeps its directory's name.
static TICKET_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)[a-z][a-z0-9]*-[0-9]+").unwrap());

/// The ticket identifier a branch (or tab) name holds, in capitals: the
/// first match, so `eng-42-fix-login` and `me/eng-42` give `ENG-42`.
pub fn ticket_in(branch: &str) -> Option<String> {
    TICKET_ID
        .find(branch)
        .map(|m| m.as_str().to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tickets_in_branch_names() {
        assert_eq!(ticket_in("eng-42-fix-login"), Some("ENG-42".into()));
        assert_eq!(ticket_in("someone/ENG-1234-thing"), Some("ENG-1234".into()));
        assert_eq!(ticket_in("feature/abc2-7"), Some("ABC2-7".into()));
        assert_eq!(ticket_in("fix-auth-eng-42"), Some("ENG-42".into()));
        assert_eq!(ticket_in("main"), None);
        assert_eq!(ticket_in("2-fast"), None);
        assert_eq!(ticket_in(""), None);
    }

    #[test]
    fn a_ticket_from_linear_s_answer() {
        let issue = json!({
            "id": "abc", "identifier": "ENG-1", "title": "Fix it",
            "url": "https://linear.app/x/issue/ENG-1", "state": { "name": "Todo" },
            "assignee": { "id": "u1", "name": "jo", "displayName": "Jo" }
        });
        let ticket = ticket_from(&issue).unwrap();
        assert_eq!(ticket.state, "Todo");
        assert_eq!(ticket.assignee.unwrap().name, "Jo");
        let unassigned =
            json!({ "id": "a", "identifier": "ENG-2", "title": "t", "assignee": null });
        assert_eq!(ticket_from(&unassigned).unwrap().assignee, None);
        assert_eq!(ticket_from(&Value::Null), None);
        assert_eq!(ticket_from(&json!({ "id": 5 })), None);
    }
}
