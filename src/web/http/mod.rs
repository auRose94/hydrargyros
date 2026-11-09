//! The HTTP layer — an expressjs-like server surface, built on
//! [axum](https://github.com/tokio-rs/axum). Routes are plain closures over
//! `Var`, JSON rides serde for free, and shared state is a cloned `Var`
//! handle. Behind the `web` feature so the core stays dependency-light.
//!
//! Two layers:
//!
//! - **[`Server`]**: `get`/`post`/`put`/`patch`/
//!   `delete`/`options` routes with `:param` and trailing `*` patterns,
//!   WebSocket routes (`open`/`message`/`close`), `mount` for static files,
//!   an `on_error` hook for unmatched requests, streamed bodies both ways
//!   (`route_stream` / [`Response::stream`]),
//!   HTTPS via the `sslCert`/`sslKey` config keys, and graceful, restartable
//!   `start()`/`stop()`.
//! - **Sugar** ([`post_var`]/[`get_var`]/[`serve`]): one-shot routers when a
//!   whole `Server` is more than you need.
//!
//! A request arrives as a `Var` object —
//! `{ "method", "path", "params", "query", "headers", "body" }` — with the
//! body pre-parsed by content type: `application/json` → `Var`,
//! `application/x-www-form-urlencoded` → object, anything else → string.
//! The helpers below (`is_json`, `accepting_json`, …) are the request
//! predicates over that object.
//!
//! ```ignore
//! // (doctests are feature-gated; see examples/web_site.rs and tests/web.rs)
//! let mut serv = Server::new(());
//! serv.get("/users/:id", |req| {
//!     let id = req.get("params").get("id").as_string().unwrap_or_default();
//!     Response::json(o!{ "id": id })
//! });
//! serv.start().await?;
//! ```

mod server;

use std::sync::Arc;

use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, Uri};
use axum::response::Response as AxumResponse;
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::object::Object;
use crate::var::Var;

pub use server::{Server, WsHandlers, WsSock};

// ---- response (status / headers / typed body) ----

/// The body flavor of a [`Response`]; picks the default content type.
enum RespBody {
    /// Serialized as JSON (`application/json`).
    Json(Var),
    /// Sent verbatim (`text/html; charset=utf-8` — express `res.send(string)`).
    Html(String),
    /// Sent verbatim (`text/plain; charset=utf-8`).
    Text(String),
    /// Sent verbatim (`application/octet-stream` unless `ct` overrides).
    Raw(Vec<u8>),
    /// Sent as chunked frames (`application/octet-stream` unless `ct`
    /// overrides) — one frame per stream item, the SSE shape.
    Stream(axum::body::Body),
}

/// A response under construction: status, extra headers, and a typed body.
/// Returned from route handlers; `Var`, `String`, and `&str` all convert
/// into one (`Var` → JSON 200, strings → HTML 200), so simple handlers can
/// return the body directly.
pub struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    ct: Option<String>,
    body: RespBody,
}

impl Response {
    /// `200` with `value` serialized as JSON.
    pub fn json(value: impl Into<Var>) -> Self {
        Response { status: 200, headers: Vec::new(), ct: None, body: RespBody::Json(value.into()) }
    }

    /// `200` HTML (express `res.send(string)` semantics).
    pub fn html(body: impl Into<String>) -> Self {
        Response { status: 200, headers: Vec::new(), ct: None, body: RespBody::Html(body.into()) }
    }

    /// `200` plain text.
    pub fn text(body: impl Into<String>) -> Self {
        Response { status: 200, headers: Vec::new(), ct: None, body: RespBody::Text(body.into()) }
    }

    /// `200` raw bytes (`application/octet-stream` unless `.ct()` overrides).
    pub fn bytes(body: impl Into<Vec<u8>>) -> Self {
        Response { status: 200, headers: Vec::new(), ct: None, body: RespBody::Raw(body.into()) }
    }

    /// `200` streamed: each stream item becomes one chunked-transfer frame
    /// (`text/event-stream` + this is SSE).
    pub fn stream<S>(source: S) -> Self
    where
        S: futures_util::TryStream + Send + 'static,
        S::Ok: Into<axum::body::Bytes>,
        S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        Response {
            status: 200,
            headers: Vec::new(),
            ct: None,
            body: RespBody::Stream(axum::body::Body::from_stream(source)),
        }
    }

    /// Override the status code.
    pub fn status(mut self, code: u16) -> Self {
        self.status = code;
        self
    }

    /// Add a response header.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Override the default content type for the body flavor.
    pub fn ct(mut self, ct: impl Into<String>) -> Self {
        self.ct = Some(ct.into());
        self
    }
}

impl axum::response::IntoResponse for Response {
    fn into_response(self) -> AxumResponse {
        let default_ct = match &self.body {
            RespBody::Json(_) => "application/json",
            RespBody::Html(_) => "text/html; charset=utf-8",
            RespBody::Text(_) => "text/plain; charset=utf-8",
            RespBody::Raw(_) | RespBody::Stream(_) => "application/octet-stream",
        };
        let Response { status, headers, ct, body } = self;
        let status = axum::http::StatusCode::from_u16(status)
            .unwrap_or(axum::http::StatusCode::OK);
        let mut res = match body {
            RespBody::Stream(b) => AxumResponse::new(b),
            other => AxumResponse::new(axum::body::Body::from(match other {
                RespBody::Json(v) => {
                    serde_json::to_vec(&v).unwrap_or_else(|_| b"null".to_vec())
                }
                RespBody::Html(s) => s.into_bytes(),
                RespBody::Text(s) => s.into_bytes(),
                RespBody::Raw(b) => b,
                RespBody::Stream(_) => unreachable!(),
            })),
        };
        *res.status_mut() = status;
        let ct = ct.unwrap_or_else(|| default_ct.to_string());
        if let Ok(v) = ct.parse() {
            res.headers_mut().insert(header::CONTENT_TYPE, v);
        }
        // user headers last, so they override the defaults
        for (k, v) in headers {
            if let (Ok(name), Ok(value)) =
                (k.parse::<HeaderName>(), v.parse::<HeaderValue>())
            {
                res.headers_mut().insert(name, value);
            }
        }
        res
    }
}

// simple handlers return the body directly: Var → JSON, strings → HTML
impl From<Var> for Response {
    fn from(v: Var) -> Self { Response::json(v) }
}
impl From<Object> for Response {
    fn from(o: Object) -> Self { Response::json(o) }
}
impl From<crate::list::List> for Response {
    fn from(l: crate::list::List) -> Self { Response::json(l) }
}
impl From<String> for Response {
    fn from(s: String) -> Self { Response::html(s) }
}
impl From<&str> for Response {
    fn from(s: &str) -> Self { Response::html(s) }
}

// ---- request helpers (predicates over the staged request) ----

/// One request header, by case-insensitive name ("" on a miss).
pub fn req_header(req: &Var, name: &str) -> String {
    req.get("headers").get(&name.to_ascii_lowercase()).as_string().unwrap_or_default()
}

/// Content type is `application/json`.
pub fn req_is_json(req: &Var) -> bool {
    req_header(req, "content-type").contains("application/json")
}

/// Content type is `application/x-www-form-urlencoded`.
pub fn req_is_www_form_urlencoded(req: &Var) -> bool {
    req_header(req, "content-type").contains("application/x-www-form-urlencoded")
}

/// The client accepts JSON (`Accept: application/json`).
pub fn req_accepting_json(req: &Var) -> bool {
    req_header(req, "accept").contains("application/json")
}

/// The client accepts HTML (`Accept: text/html`).
pub fn req_accepting_html(req: &Var) -> bool {
    req_header(req, "accept").contains("text/html")
}

// ---- body/query decoding ----

/// Percent-decode a path segment (`%20` → space; `+` stays `+`).
pub(crate) fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => match bytes.get(i + 1..i + 3).and_then(|h| {
                std::str::from_utf8(h).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
            }) {
                Some(b) => { out.push(b); i += 3; }
                None => { out.push(b'%'); i += 1; }
            },
            b => { out.push(b); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Decode a query/form component: `+` means space, then percent-decoding.
fn form_decode(s: &str) -> String {
    percent_decode(&s.replace('+', " "))
}

/// Parse `a=1&b=x%20y` into an ordered object.
pub(crate) fn parse_query(q: &str) -> Object {
    let o = Object::new();
    for pair in q.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        o.set(form_decode(k), Var::from(form_decode(v)));
    }
    o
}

/// Build the request `Var`: method, path, params, query, headers
/// (lowercased names), and a content-typed body.
pub(crate) fn build_req(
    method: &Method,
    uri: &Uri,
    params: impl IntoIterator<Item = (String, String)>,
    query: Option<&str>,
    headers: &HeaderMap,
    body: &[u8],
) -> Var {
    let req = Object::new();
    req.set("method", Var::from(method.as_str()));
    req.set("path", Var::from(uri.path()));

    let params_obj = Object::new();
    for (k, v) in params {
        let key = if k == "rest" { "*".to_string() } else { k }; // `{*rest}` → params["*"]
        params_obj.set(key, Var::from(percent_decode(&v)));
    }
    req.set("params", Var::from(params_obj));

    req.set("query", match query {
        Some(q) if !q.is_empty() => Var::from(parse_query(q)),
        _ => Var::from(Object::new()),
    });

    let headers_obj = Object::new();
    for (name, value) in headers {
        let v = value.to_str().unwrap_or_default().to_string();
        headers_obj.set(name.as_str().to_ascii_lowercase(), Var::from(v));
    }
    req.set("headers", Var::from(headers_obj));

    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let body_var = if ct.contains("application/json") {
        serde_json::from_slice::<Var>(body).unwrap_or(Var::Null)
    } else if ct.contains("application/x-www-form-urlencoded") {
        Var::from(parse_query(&String::from_utf8_lossy(body)))
    } else {
        Var::from(String::from_utf8_lossy(body).into_owned())
    };
    req.set("body", body_var);
    Var::from(req)
}

// ---- sugar (one-shot routers) ----

/// Register a POST route: the JSON body is parsed into a `Var` and handed to
/// the handler; its return value (anything `Into<Response>`) is the reply.
/// Malformed JSON is rejected by axum with a 4xx before the handler runs —
/// this is the strict-JSON sugar; [`Server`] stages the raw request instead.
pub fn post_var<F, R>(path: &str, f: F) -> Router
where
    F: Fn(Var) -> R + Send + Sync + 'static,
    R: Into<Response>,
{
    let f = Arc::new(f);
    Router::new().route(
        path,
        post(move |Json(body): Json<Var>| {
            let f = f.clone();
            async move {
                let reply: Response = f(body).into();
                reply
            }
        }),
    )
}

/// Register a GET route: the handler runs with no input, its return value
/// (anything `Into<Response>`) is the reply.
pub fn get_var<F, R>(path: &str, f: F) -> Router
where
    F: Fn() -> R + Send + Sync + 'static,
    R: Into<Response>,
{
    let f: Arc<dyn Fn() -> Response + Send + Sync> = Arc::new(move || f().into());
    Router::new().route(path, get(move || {
        let f = f.clone();
        async move { f() }
    }))
}

/// Bind `addr` and serve `router` — the express `app.listen()` equivalent.
/// For graceful stop and restarts, use [`Server::start`]/[`Server::stop`].
pub async fn serve(router: Router, addr: &str) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await
}
