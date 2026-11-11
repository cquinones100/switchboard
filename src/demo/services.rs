//! Services of a test's own on 127.0.0.1, each as strict as the real one
//! about how it is asked, so a test passes only when the code asks rightly:
//! GitHub's REST API and GraphQL endpoint, Linear's GraphQL endpoint, and
//! Slack's Web API each refuse the wrong method, a path they don't have, a
//! missing or wrong token and a body they can't read, with the real
//! service's status and the shape of its error, before the test's answer is
//! asked. The answer gives `None` for what the service has no such thing
//! of, which it answers as the service does (GitHub's 404, ...). A test that
//! answers with an error on purpose, to show how the code takes it, still
//! goes through the same checks first.
//!
//! Every request it reads is kept, refused or not, for the test to look at.
//! A connection carries one request, answered with `Connection: close`.
//! Since the code often takes a refusal quietly (a background lookup that
//! failed, a view that shows nothing), a fake that refused a request fails
//! the test when it is dropped, unless the test said it means to make
//! wrong requests (`expect_refusals`).
//!
//! Used by `switchboard demo`, which answers through it with its made-up
//! data (`super::fixtures`), keeping what it refused in a log rather than
//! failing; by the integration tests as `common::http`; and by the
//! library's own unit tests as `crate::fake_http`. It uses nothing but std
//! and serde_json. tests/it/stand_ins.rs shows each refusing a wrong request.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// A request as the fake read it.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    /// The path and query, as sent.
    pub target: String,
    /// Each header, its name in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// The path, without the query.
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("")
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The body as JSON, `Null` when it isn't.
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// A GraphQL request's query, `""` for none.
    pub fn query(&self) -> String {
        self.json()["query"].as_str().unwrap_or("").to_string()
    }
}

/// An answer.
#[derive(Clone, Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn json(status: u16, body: Value) -> Reply {
        Reply::bytes(status, "application/json; charset=utf-8", body.to_string())
    }

    pub fn bytes(status: u16, kind: &str, body: impl Into<Vec<u8>>) -> Reply {
        Reply {
            status,
            headers: vec![("Content-Type".into(), kind.into())],
            body: body.into(),
        }
    }

    /// The answer with another header.
    pub fn header(mut self, name: &str, value: impl Into<String>) -> Reply {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// Which service a fake is, which says what it refuses before the test's
/// answer is asked, and how it says it has no such thing.
#[derive(Clone, Debug)]
pub enum Service {
    /// GitHub's API: GraphQL at `POST /graphql`, REST under `/`, every
    /// request with a User-Agent and the token as `Authorization: Bearer
    /// <token>` (or `token <token>`). Its repositories are private, so a
    /// REST request with no token finds nothing, as GitHub answers one.
    GitHub { token: String },
    /// GitHub's REST API for a public repository (Switchboard's releases),
    /// which needs no token, though one sent must be right, and serves the
    /// release's files too.
    GitHubPublic { token: Option<String> },
    /// Linear's GraphQL endpoint, `POST /graphql`, with the personal API
    /// key as the whole `Authorization` header (no `Bearer`).
    Linear { key: String },
    /// Slack's Web API, `/api/<method>`, with the bot token as a Bearer.
    Slack { token: String },
    /// Any web server: what the test answers, and a 404 for the rest.
    Web,
}

/// The versions of GitHub's REST API there are, for `X-GitHub-Api-Version`.
const GITHUB_API_VERSIONS: &[&str] = &["2022-11-28", "2026-03-10"];

/// GitHub's error, as its REST API and GraphQL endpoint give one.
fn github_error(status: u16, message: &str, docs: &str) -> Reply {
    Reply::json(
        status,
        json!({
            "message": message,
            "documentation_url": format!("https://docs.github.com/{docs}"),
            "status": status.to_string(),
        }),
    )
}

/// Linear's error, which it gives with a 400 whatever went wrong.
fn linear_error(message: &str, code: &str, presentable: &str) -> Reply {
    Reply::json(
        400,
        json!({ "errors": [{
            "message": message,
            "extensions": {
                "type": if code == "AUTHENTICATION_ERROR" { "authentication error" } else { "invalid input" },
                "code": code,
                "statusCode": if code == "AUTHENTICATION_ERROR" { 401 } else { 400 },
                "userPresentableMessage": presentable,
            }
        }] }),
    )
}

/// Slack's refusal: always a 200, saying why in `error`.
fn slack_error(error: &str) -> Reply {
    Reply::json(200, json!({ "ok": false, "error": error }))
}

impl Service {
    /// What the service answers before the test is asked, when the request
    /// isn't one it takes.
    fn refusal(&self, request: &Request) -> Option<Reply> {
        match self {
            Service::GitHub { token } => github_refusal(request, Some(token), true),
            Service::GitHubPublic { token } => github_refusal(request, token.as_ref(), false),
            Service::Linear { key } => linear_refusal(request, key),
            Service::Slack { token } => slack_refusal(request, token),
            Service::Web => None,
        }
    }

    /// What it says when it has no such thing.
    fn not_found(&self, request: &Request) -> Reply {
        match self {
            Service::GitHub { .. } | Service::GitHubPublic { .. } => {
                github_error(404, "Not Found", "rest")
            }
            Service::Linear { .. } => Reply::bytes(
                404,
                "text/html; charset=utf-8",
                format!("Cannot {} {}\n", request.method, request.path()),
            ),
            Service::Slack { .. } => Reply::json(
                404,
                json!({ "ok": false, "error": "unknown_method", "req_method": request.path().trim_start_matches("/api/") }),
            ),
            Service::Web => Reply::bytes(404, "text/plain", "Not Found\n"),
        }
    }
}

fn github_refusal(request: &Request, token: Option<&String>, private: bool) -> Option<Reply> {
    // GitHub refuses a request with no User-Agent before anything else.
    if request.header("user-agent").is_none_or(str::is_empty) {
        return Some(Reply::bytes(
            403,
            "text/plain; charset=utf-8",
            "Request forbidden by administrative rules. Please make sure your request has a User-Agent header (https://docs.github.com/en/rest/overview/resources-in-the-rest-api#user-agent-required). Check https://developer.github.com for other possible causes.\n",
        ));
    }
    if let Some(version) = request.header("x-github-api-version")
        && !GITHUB_API_VERSIONS.contains(&version)
    {
        return Some(github_error(
            400,
            &format!("API version {version} is not supported."),
            "rest/about-the-rest-api/api-versions",
        ));
    }
    let graphql = request.path() == "/graphql";
    match request.header("authorization") {
        Some(given) => {
            let right = token.is_some_and(|t| {
                given == format!("Bearer {t}")
                    || given == format!("bearer {t}")
                    || given == format!("token {t}")
            });
            if !right {
                return Some(github_error(
                    401,
                    "Bad credentials",
                    if graphql { "graphql" } else { "rest" },
                ));
            }
        }
        None if graphql => {
            return Some(github_error(
                401,
                "This endpoint requires you to be authenticated.",
                "graphql/guides/forming-calls-with-graphql#authenticating-with-graphql",
            ));
        }
        // A private repository is one GitHub doesn't show without a token.
        None if private => return Some(github_error(404, "Not Found", "rest")),
        None => {}
    }
    if graphql {
        if request.method != "POST" {
            return Some(github_error(404, "Not Found", "graphql"));
        }
        let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
            return Some(github_error(400, "Problems parsing JSON", "graphql"));
        };
        if !body["query"].is_string() {
            return Some(Reply::json(
                200,
                json!({ "errors": [{ "message": "A query attribute must be specified and must be a string." }] }),
            ));
        }
        if !(body["variables"].is_object() || body["variables"].is_null()) {
            return Some(Reply::json(
                200,
                json!({ "errors": [{ "message": "Variables are invalid JSON." }] }),
            ));
        }
        return None;
    }
    if matches!(request.method.as_str(), "POST" | "PATCH" | "PUT")
        && !request.body.is_empty()
        && serde_json::from_slice::<Value>(&request.body).is_err()
    {
        return Some(github_error(400, "Problems parsing JSON", "rest"));
    }
    if !matches!(
        request.method.as_str(),
        "GET" | "HEAD" | "POST" | "PATCH" | "PUT" | "DELETE"
    ) {
        return Some(github_error(404, "Not Found", "rest"));
    }
    None
}

fn linear_refusal(request: &Request, key: &str) -> Option<Reply> {
    if request.path() != "/graphql" {
        return Some(Service::Linear { key: key.into() }.not_found(request));
    }
    if request.header("authorization") != Some(key) {
        return Some(linear_error(
            "Authentication required, not authenticated",
            "AUTHENTICATION_ERROR",
            "You need to authenticate to access this operation.",
        ));
    }
    if request.method != "POST" {
        return Some(linear_error(
            "GraphQL operations must be sent as a POST",
            "BAD_USER_INPUT",
            "This operation can only be sent as a POST.",
        ));
    }
    if !request
        .header("content-type")
        .is_some_and(|t| t.starts_with("application/json"))
    {
        return Some(linear_error(
            "Must provide query string.",
            "BAD_USER_INPUT",
            "Must provide query string.",
        ));
    }
    match serde_json::from_slice::<Value>(&request.body) {
        Ok(body) if body["query"].is_string() => None,
        _ => Some(linear_error(
            "Must provide query string.",
            "BAD_USER_INPUT",
            "Must provide query string.",
        )),
    }
}

fn slack_refusal(request: &Request, token: &str) -> Option<Reply> {
    let Some(method) = request.path().strip_prefix("/api/") else {
        return Some(Reply::bytes(404, "text/html", "Not Found\n"));
    };
    if method.is_empty() {
        return Some(slack_error("unknown_method"));
    }
    if !matches!(request.method.as_str(), "GET" | "POST") {
        return Some(slack_error("invalid_request"));
    }
    // The token as a Bearer, or as the form's `token`.
    let in_form = request
        .text()
        .split('&')
        .find_map(|pair| pair.strip_prefix("token="))
        .map(str::to_string);
    let given = request
        .header("authorization")
        .and_then(|a| a.strip_prefix("Bearer "))
        .map(str::to_string)
        .or(in_form);
    match given {
        None => Some(slack_error("not_authed")),
        Some(t) if t != token => Some(slack_error("invalid_auth")),
        Some(_) => None,
    }
}

type Answer = dyn Fn(&Request) -> Option<Reply> + Send + Sync;

/// A service on 127.0.0.1, answering on a thread of its own until the test
/// ends.
pub struct Fake {
    /// Its address, `http://127.0.0.1:<port>`, with no `/` at the end.
    pub url: String,
    seen: Arc<Mutex<Vec<Request>>>,
    refused: Arc<Mutex<Vec<String>>>,
    refusals_expected: bool,
}

impl Fake {
    /// `service`, answering what it takes with `answer`.
    pub fn start(
        service: Service,
        answer: impl Fn(&Request) -> Option<Reply> + Send + Sync + 'static,
    ) -> Fake {
        Fake::start_on(0, service, answer)
    }

    /// `service` on `port` when it is free (the demo's services started
    /// again where its server knows them), else on any.
    pub fn start_on(
        port: u16,
        service: Service,
        answer: impl Fn(&Request) -> Option<Reply> + Send + Sync + 'static,
    ) -> Fake {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .or_else(|_| TcpListener::bind("127.0.0.1:0"))
            .expect("a port");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<Request>>> = Arc::default();
        let refused: Arc<Mutex<Vec<String>>> = Arc::default();
        let (log, wrong) = (seen.clone(), refused.clone());
        let answer: Arc<Answer> = Arc::new(answer);
        let service = Arc::new(service);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let (log, wrong, answer, service) =
                    (log.clone(), wrong.clone(), answer.clone(), service.clone());
                std::thread::spawn(move || serve(stream, &service, &*answer, &log, &wrong));
            }
        });
        Fake {
            url,
            seen,
            refused,
            refusals_expected: false,
        }
    }

    /// GitHub, its token `token`.
    pub fn github(
        token: &str,
        answer: impl Fn(&Request) -> Option<Reply> + Send + Sync + 'static,
    ) -> Fake {
        Fake::start(
            Service::GitHub {
                token: token.into(),
            },
            answer,
        )
    }

    /// Linear, its key `key`.
    pub fn linear(
        key: &str,
        answer: impl Fn(&Request) -> Option<Reply> + Send + Sync + 'static,
    ) -> Fake {
        Fake::start(Service::Linear { key: key.into() }, answer)
    }

    /// `service` answering what it takes with each of `replies` in turn,
    /// and as it has no such thing once they are spent.
    pub fn canned(service: Service, replies: Vec<Reply>) -> Fake {
        let left = Mutex::new(std::collections::VecDeque::from(replies));
        Fake::start(service, move |_| left.lock().unwrap().pop_front())
    }

    /// Its GraphQL endpoint (GitHub's or Linear's).
    pub fn graphql_url(&self) -> String {
        format!("{}/graphql", self.url)
    }

    /// Every request it has read, refused ones too.
    pub fn seen(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }

    /// The requests it refused, each as its method, target and answer.
    pub fn refused(&self) -> Vec<String> {
        self.refused.lock().unwrap().clone()
    }

    /// Takes the refusals kept so far and forgets every request read, for a
    /// fake that runs on (the demo's), which would otherwise keep them all.
    pub fn take_refused(&self) -> Vec<String> {
        self.seen.lock().unwrap().clear();
        std::mem::take(&mut *self.refused.lock().unwrap())
    }

    /// The test means to make wrong requests of it, to see them refused.
    pub fn expect_refusals(&mut self) {
        self.refusals_expected = true;
    }
}

impl Drop for Fake {
    /// Fails the test if a request was refused that the test didn't mean to
    /// make: the code asked in a way the real service would refuse too.
    fn drop(&mut self) {
        let refused = self.refused();
        if !self.refusals_expected && !refused.is_empty() && !std::thread::panicking() {
            panic!(
                "the stand-in service at {} refused a request the code made, as the real one would:\n{}",
                self.url,
                refused.join("\n")
            );
        }
    }
}

fn serve(
    stream: TcpStream,
    service: &Service,
    answer: &Answer,
    seen: &Mutex<Vec<Request>>,
    refused: &Mutex<Vec<String>>,
) {
    // A connection that sends nothing is given up, not waited on for good.
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(10)));
    // Everything read from the connection is bounded, its request line and
    // headers as well as its body.
    let mut reader = BufReader::new(stream.take((MOST_BODY + MOST_HEAD) as u64));
    let Some(request) = read_request(&mut reader) else {
        return;
    };
    seen.lock().unwrap().push(request.clone());
    let reply = match service.refusal(&request) {
        Some(refusal) => {
            refused.lock().unwrap().push(format!(
                "{} {} -> {} {}",
                request.method,
                request.target,
                refusal.status,
                String::from_utf8_lossy(&refusal.body).trim()
            ));
            refusal
        }
        None => answer(&request).unwrap_or_else(|| service.not_found(&request)),
    };
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reason(reply.status),
        reply.body.len()
    );
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let mut stream = reader.into_inner().into_inner();
    let _ = stream.write_all(head.as_bytes());
    if request.method != "HEAD" {
        let _ = stream.write_all(&reply.body);
    }
    let _ = stream.flush();
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

/// The longest body a request may have: far past any the workspace sends,
/// so a process on the computer can't have the demo's services allocate
/// without end.
const MOST_BODY: usize = 16 * 1024 * 1024;
/// The most a request's line and headers may take together.
const MOST_HEAD: usize = 64 * 1024;

/// One HTTP/1.1 request: its line, its headers, and its body by its length
/// or in chunks.
fn read_request(reader: &mut impl BufRead) -> Option<Request> {
    let mut first = String::new();
    reader.read_line(&mut first).ok()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    let header = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    };
    let mut body = Vec::new();
    if header("transfer-encoding").is_some_and(|t| t.eq_ignore_ascii_case("chunked")) {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size).ok()?;
            let size = usize::from_str_radix(size.trim().split(';').next()?, 16).ok()?;
            if body.len().saturating_add(size) > MOST_BODY {
                return None;
            }
            let mut chunk = vec![0; size + 2];
            reader.read_exact(&mut chunk).ok()?;
            if size == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..size]);
        }
    } else if let Some(length) = header("content-length").and_then(|l| l.parse().ok()) {
        if length > MOST_BODY {
            return None;
        }
        body = vec![0; length];
        reader.read_exact(&mut body).ok()?;
    }
    Some(Request {
        method,
        target,
        headers,
        body,
    })
}
