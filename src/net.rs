//! The one way Switchboard reaches a service over HTTPS: a shared client
//! (reqwest on rustls, so nothing has to be installed for TLS), JSON in and
//! out, and every request counted in the request audit (`requests::count`) as
//! it is sent, whether or not an answer comes. It replaces the tmux version's
//! `gh` and `curl` calls, each of which was a process the security agent
//! inspected, with requests made in-process.
//!
//! The client is shared by every request made on the same async runtime, so
//! connections and TLS sessions are kept between them. One per runtime rather
//! than one for the process, because hyper's pooled connections belong to the
//! runtime that opened them: a command that runs a short-lived runtime of its
//! own (`util::block_on`) would otherwise leave the next one a pool of
//! connections whose tasks are gone, and its requests would fail with
//! "dispatch task is gone".
//!
//! A service's address can be moved by an environment variable
//! (`SWITCHBOARD_GITHUB_API`, `SWITCHBOARD_GITHUB_GRAPHQL`,
//! `SWITCHBOARD_LINEAR_API`), which is how the tests point it at a server of
//! their own on 127.0.0.1.
//!
//! Built without the `net` feature, every function here is still there and
//! fails with a `ServiceError` saying Switchboard was built without network
//! support, so no caller needs a `cfg` of its own.

use std::fmt;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

#[cfg(feature = "net")]
use crate::requests;

/// The user agent every request carries. GitHub refuses a request without
/// one, and a name of our own lets a service tell whose requests these are.
pub const USER_AGENT: &str = concat!(
    "Switchboard/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/cquinones100/switchboard)"
);

/// How long a connection may take to open, and a whole request to be
/// answered. Long enough for a slow GraphQL search, short enough that a lookup
/// stuck on a dead network gives up well before the next one is due.
#[cfg(feature = "net")]
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(feature = "net")]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// What made a request, for the request audit: who answers (`service`, as
/// "GitHub" or "Linear"), how it is reached (`protocol`, "GraphQL" or
/// "REST"), and what asked (`source`, as "tab name: PR").
#[derive(Clone, Copy, Debug)]
pub struct Audit<'a> {
    pub service: &'a str,
    pub protocol: &'a str,
    pub source: &'a str,
}

/// Why a request to a service failed, in kinds a caller can act on: a view
/// saying how to set an integration up wants to tell a missing token from a
/// service that is down. Every error from here and `integrations` is one of
/// these, reached with `err.downcast_ref::<ServiceError>()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The service is turned off in Settings.
    Off,
    /// No token or key to send.
    NoCredentials,
    /// The service refused the token or key it was sent.
    Unauthorized,
    /// The service's rate limit is spent; `reset_at` is when it comes back,
    /// in seconds since the epoch, when the service said.
    RateLimited { reset_at: Option<i64> },
    /// What was asked for isn't there, or can't be seen with this token.
    NotFound,
    /// The service couldn't be reached at all (or this build can't reach
    /// anything).
    Unreachable,
    /// The service answered with an error of another kind.
    Failed,
}

/// A failed request: its kind, the service, and a message for the person,
/// the service's own words where it gave any.
#[derive(Clone, Debug)]
pub struct ServiceError {
    pub kind: ErrorKind,
    pub service: String,
    pub message: String,
}

impl ServiceError {
    pub fn new(kind: ErrorKind, service: &str, message: impl Into<String>) -> ServiceError {
        ServiceError {
            kind,
            service: service.to_string(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ServiceError {}

/// The kind of a failure from here, or `None` for an error of another sort.
pub fn kind_of(err: &anyhow::Error) -> Option<&ErrorKind> {
    err.downcast_ref::<ServiceError>().map(|e| &e.kind)
}

/// A service's answer: its status, its headers (names in lower case), and
/// its body read as JSON. A body that isn't JSON (a proxy's HTML error page)
/// is kept as a JSON string of its text, and an empty one is `null`, so a
/// caller always has something to look at and nothing here fails on what a
/// service sends back.
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl Response {
    /// Whether the status is a success (2xx).
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// A header's value, by its name in any case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A service's address: the environment variable `var` when it is set and
/// not empty, else `default`, without a trailing `/`.
pub fn base_url(var: &str, default: &str) -> String {
    match std::env::var(var) {
        Ok(url) if !url.trim().is_empty() => url.trim().trim_end_matches('/').to_string(),
        _ => default.trim_end_matches('/').to_string(),
    }
}

/// A GET, its answer read as JSON.
pub async fn get_json(url: &str, headers: &[(&str, &str)], audit: Audit<'_>) -> Result<Response> {
    request_json("GET", url, headers, None, audit).await
}

/// A POST of `body` as JSON, its answer read as JSON.
pub async fn post_json(
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
    audit: Audit<'_>,
) -> Result<Response> {
    request_json("POST", url, headers, Some(body), audit).await
}

/// A request with any method, with `body` sent as JSON when there is one.
/// Counted in the request audit as it is sent. Fails only when no answer
/// comes (the network, a timeout, a method that isn't one); an answer with an
/// error status is an `Ok` for the caller to read, since each service says
/// what went wrong in its own way.
#[cfg(feature = "net")]
pub async fn request_json(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&Value>,
    audit: Audit<'_>,
) -> Result<Response> {
    let method =
        reqwest::Method::from_bytes(method.to_ascii_uppercase().as_bytes()).map_err(|_| {
            ServiceError::new(
                ErrorKind::Failed,
                audit.service,
                format!("{method} isn't an HTTP method"),
            )
        })?;
    let mut request = client()?.request(method, url);
    // JSON is asked for unless the caller names a kind of its own (GitHub's
    // `application/vnd.github+json`); headers add rather than replace, so
    // both would be sent.
    if !headers
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("accept"))
    {
        request = request.header("Accept", "application/json");
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    if let Some(body) = body {
        request = request.json(body);
    }
    requests::count(audit.service, audit.protocol, audit.source);
    let unreachable = |e: reqwest::Error| {
        let why = if e.is_timeout() {
            "it took too long to answer".to_string()
        } else {
            // reqwest's own words are a chain ("error sending request for
            // url (...)": the cause), and the cause is the useful part.
            let mut why = e.to_string();
            let mut source = std::error::Error::source(&e);
            while let Some(cause) = source {
                why = cause.to_string();
                source = cause.source();
            }
            why
        };
        ServiceError::new(
            ErrorKind::Unreachable,
            audit.service,
            format!("Couldn't reach {}: {why}", audit.service),
        )
    };
    let response = request.send().await.map_err(unreachable)?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let bytes = response.bytes().await.map_err(unreachable)?;
    Ok(Response {
        status,
        headers,
        body: parse_body(&bytes),
    })
}

/// Without the `net` feature: the same function, which never reaches out.
#[cfg(not(feature = "net"))]
pub async fn request_json(
    _method: &str,
    _url: &str,
    _headers: &[(&str, &str)],
    _body: Option<&Value>,
    audit: Audit<'_>,
) -> Result<Response> {
    Err(ServiceError::new(
        ErrorKind::Unreachable,
        audit.service,
        format!(
            "Switchboard was built without network support, so it can't reach {}.",
            audit.service
        ),
    )
    .into())
}

/// A GET whose answer is a file rather than JSON (a release's archive): its
/// status and its bytes, redirects followed (GitHub sends a release's files
/// from another host), given `timeout` to arrive whole rather than the
/// usual 30 seconds, since a file can be large and a connection slow.
/// Counted in the request audit as it is sent, and failing only when no
/// answer comes, as `request_json` does.
#[cfg(feature = "net")]
pub async fn get_bytes(
    url: &str,
    headers: &[(&str, &str)],
    timeout: Duration,
    audit: Audit<'_>,
) -> Result<(u16, Vec<u8>)> {
    let mut request = client()?.get(url).timeout(timeout);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    requests::count(audit.service, audit.protocol, audit.source);
    let unreachable = |e: reqwest::Error| {
        let why = if e.is_timeout() {
            "it took too long to arrive".to_string()
        } else {
            let mut why = e.to_string();
            let mut source = std::error::Error::source(&e);
            while let Some(cause) = source {
                why = cause.to_string();
                source = cause.source();
            }
            why
        };
        ServiceError::new(
            ErrorKind::Unreachable,
            audit.service,
            format!("Couldn't download from {}: {why}", audit.service),
        )
    };
    let response = request.send().await.map_err(unreachable)?;
    let status = response.status().as_u16();
    let bytes = response.bytes().await.map_err(unreachable)?;
    Ok((status, bytes.to_vec()))
}

/// Without the `net` feature: the same function, which never reaches out.
#[cfg(not(feature = "net"))]
pub async fn get_bytes(
    _url: &str,
    _headers: &[(&str, &str)],
    _timeout: Duration,
    audit: Audit<'_>,
) -> Result<(u16, Vec<u8>)> {
    Err(ServiceError::new(
        ErrorKind::Unreachable,
        audit.service,
        format!(
            "Switchboard was built without network support, so it can't download from {}.",
            audit.service
        ),
    )
    .into())
}

/// A body as JSON, else its text as a JSON string, else `null` when empty.
pub fn parse_body(bytes: &[u8]) -> Value {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Value::Null;
    }
    serde_json::from_slice(bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(bytes).trim().to_string()))
}

#[cfg(feature = "net")]
mod shared {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};

    use anyhow::{Result, anyhow};
    use tokio::runtime::{Handle, Id};

    use super::{CONNECT_TIMEOUT, REQUEST_TIMEOUT, USER_AGENT};

    // A client per runtime, as the module's comment says why. A command's
    // short runtimes come and go, so the map is emptied once it holds more
    // than a handful; the long-running server's own runtime just builds its
    // client again then, which costs a new connection, no more.
    static CLIENTS: LazyLock<Mutex<HashMap<Id, reqwest::Client>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    const KEEP: usize = 8;

    pub fn client() -> Result<reqwest::Client> {
        let runtime = Handle::try_current()
            .map_err(|_| anyhow!("a request needs an async runtime to run on"))?
            .id();
        let mut clients = CLIENTS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(client) = clients.get(&runtime) {
            return Ok(client.clone());
        }
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| anyhow!("couldn't set up HTTPS: {e}"))?;
        if clients.len() >= KEEP {
            clients.clear();
        }
        clients.insert(runtime, client.clone());
        Ok(client)
    }
}

#[cfg(feature = "net")]
use shared::client;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies_that_are_not_json_are_kept_as_text() {
        assert_eq!(parse_body(b"{\"a\":1}"), serde_json::json!({"a": 1}));
        assert_eq!(
            parse_body(b"<html>Bad gateway</html>\n"),
            Value::String("<html>Bad gateway</html>".into())
        );
        assert_eq!(parse_body(b""), Value::Null);
        assert_eq!(parse_body(b" \n"), Value::Null);
    }

    #[test]
    fn headers_are_found_in_any_case() {
        let response = Response {
            status: 403,
            headers: vec![("x-ratelimit-remaining".into(), "0".into())],
            body: Value::Null,
        };
        assert_eq!(response.header("X-RateLimit-Remaining"), Some("0"));
        assert!(!response.ok());
    }
}
