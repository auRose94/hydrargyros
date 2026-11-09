//! `Server` — the expressjs-like server facade: routes buffer onto the
//! server (`get`/`post`/`put`/`patch`/`delete`/`options`, plus `ws` for
//! WebSockets, `mount` for static files and `on_error` for the 404-page
//! hook) until `start()` runs them. `stop()` is graceful and safe from any
//! thread, and a stopped server can be started again.
//!
//! Route patterns follow uWS-style conventions:
//! `:id` captures one path segment, a trailing `*` captures the rest
//! including slashes (surfacing as `params["*"]`). Unmatched requests fall
//! through the mounts, then the `on_error` handler, then an automatic 404.

use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{RawPathParams, RawQuery};
use axum::http::{HeaderMap, Method, Uri};
use axum::routing::{delete, get, options, patch, post, put};
use axum::{body::Bytes, Router};
use axum_server::Handle;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use super::{build_req, Response};
use crate::var::Var;

type RouteHandler = Arc<dyn Fn(Var) -> Response + Send + Sync>;

/// How long `stop()` waits for in-flight requests before force-closing them.
const GRACE_PERIOD: std::time::Duration = std::time::Duration::from_secs(5);

/// The parts of the server that handlers touch at request time: the mount
/// table and the error hook. Shared behind an `Arc` so `start()` can serve
/// while `mount`/`on_error` stay plain methods.
#[derive(Default)]
struct Shared {
    mounts: Mutex<Vec<(String, PathBuf)>>,
    error_handler: Mutex<Option<RouteHandler>>,
}

/// The HTTP server facade, transport-independent.
///
/// Cloning a `Server` shares the routes, mounts, and stop signal — hand a
/// clone to `tokio::spawn` and keep the original for `stop()`.
#[derive(Clone)]
pub struct Server {
    addr: String,
    /// The `sslCert`/`sslKey` PEM pair — when present, `start()` serves HTTPS
    /// on the same port (one listener, both schemes).
    tls: Option<(PathBuf, PathBuf)>,
    router: Router,
    shared: Arc<Shared>,
    /// Graceful-stop signal for the currently running `start()`; `None`
    /// while stopped, replaced on each start so servers restart cleanly.
    shutdown: Arc<Mutex<Option<tokio::sync::watch::Sender<bool>>>>,
    /// The addr the running (or last) start() actually bound (resolves `:0`).
    bound: Arc<Mutex<String>>,
}

impl Default for Server {
    fn default() -> Self { Self::new() }
}

impl Server {
    /// A server on the defaults: `127.0.0.1:8080`.
    pub fn new() -> Self { Self::with_config(()) }

    /// Configure from data:
    /// `o!{ "host": "127.0.0.1", "port": 8080 }`. `Null` means defaults.
    /// A `"sslCert"`/`"sslKey"` PEM pair switches the vhost to HTTPS.
    pub fn with_config(config: impl Into<Var>) -> Self {
        let cfg = config.into();
        let host = cfg.get("host").as_string().unwrap_or_else(|| "127.0.0.1".to_string());
        let port = cfg.get("port").as_i64().unwrap_or(8080);
        let tls = match (
            cfg.get("sslCert").as_string(),
            cfg.get("sslKey").as_string(),
        ) {
            (Some(cert), Some(key)) => Some((PathBuf::from(cert), PathBuf::from(key))),
            _ => None,
        };
        let shared = Arc::new(Shared::default());

        // Unmatched requests fall through in order: mounts, then the error
        // hook, then an automatic 404.
        let fb = shared.clone();
        let router = Router::new().fallback(
            move |method: Method, uri: Uri, headers: HeaderMap, bytes: Bytes| {
                let fb = fb.clone();
                async move {
                    let req = build_req(
                        &method,
                        &uri,
                        std::iter::empty::<(String, String)>(),
                        uri.query(),
                        &headers,
                        &bytes,
                    );
                    let mounts = fb.mounts.lock().unwrap();
                    for (prefix, dir) in mounts.iter() {
                        let rest = if prefix == "/" {
                            Some(uri.path())
                        } else {
                            uri.path().strip_prefix(prefix.as_str()).filter(|r| r.starts_with('/'))
                        };
                        if let Some(file) = rest.and_then(|r| serve_file(dir, r.trim_start_matches('/'))) {
                            return file;
                        }
                    }
                    drop(mounts);
                    if let Some(on_error) = fb.error_handler.lock().unwrap().as_ref() {
                        return on_error(req);
                    }
                    Response::text("Not Found").status(404)
                }
            },
        );

        Server {
            addr: format!("{host}:{port}"),
            tls,
            router,
            shared,
            shutdown: Arc::new(Mutex::new(None)),
            bound: Arc::new(Mutex::new(String::new())),
        }
    }

    // ---- route buffering (get/post/put/patch/delete/options) ----

    /// Buffer a GET route: `(pattern, func)`.
    pub fn get<F, R>(&mut self, pattern: &str, f: F) -> &mut Self
    where
        F: Fn(Var) -> R + Send + Sync + 'static,
        R: Into<Response>,
    {
        self.route("GET", pattern, f)
    }

    pub fn post<F, R>(&mut self, pattern: &str, f: F) -> &mut Self
    where
        F: Fn(Var) -> R + Send + Sync + 'static,
        R: Into<Response>,
    {
        self.route("POST", pattern, f)
    }

    pub fn put<F, R>(&mut self, pattern: &str, f: F) -> &mut Self
    where
        F: Fn(Var) -> R + Send + Sync + 'static,
        R: Into<Response>,
    {
        self.route("PUT", pattern, f)
    }

    pub fn patch<F, R>(&mut self, pattern: &str, f: F) -> &mut Self
    where
        F: Fn(Var) -> R + Send + Sync + 'static,
        R: Into<Response>,
    {
        self.route("PATCH", pattern, f)
    }

    /// The DELETE verb, spelled out.
    pub fn delete<F, R>(&mut self, pattern: &str, f: F) -> &mut Self
    where
        F: Fn(Var) -> R + Send + Sync + 'static,
        R: Into<Response>,
    {
        self.route("DELETE", pattern, f)
    }

    pub fn options<F, R>(&mut self, pattern: &str, f: F) -> &mut Self
    where
        F: Fn(Var) -> R + Send + Sync + 'static,
        R: Into<Response>,
    {
        self.route("OPTIONS", pattern, f)
    }

    /// Any verb, by name — the general form behind the per-method sugar.
    pub fn route<F, R>(&mut self, method: &str, pattern: &str, f: F) -> &mut Self
    where
        F: Fn(Var) -> R + Send + Sync + 'static,
        R: Into<Response>,
    {
        let handler: RouteHandler = Arc::new(move |req: Var| f(req).into());
        let ap = axum_pattern(pattern);
        let taken = std::mem::take(&mut self.router);
        self.router = taken.route(&ap, method_router(method, handler));
        self
    }

    /// Streaming variant of [`route`](Self::route) — the request body as a
    /// Rust stream. The async handler receives the staged request (its
    /// `body` is `Null`; the raw body is the second argument) and the body
    /// stream, so uploads can be consumed chunk by chunk without buffering.
    pub fn route_stream<F, Fut, R>(&mut self, method: &str, pattern: &str, f: F) -> &mut Self
    where
        F: Fn(Var, axum::body::Body) -> Fut + Clone + Send + Sync + 'static,
        Fut: std::future::Future<Output = R> + Send + 'static,
        R: Into<Response>,
    {
        let ap = axum_pattern(pattern);
        let taken = std::mem::take(&mut self.router);
        self.router = taken.route(&ap, method_router_stream(method, f));
        self
    }

    /// Buffer a WebSocket route with `{open, message, close}` handlers:
    /// `open(sock)` fires on connect, `message(sock, data)` per text message,
    /// `close(sock)` on teardown. Push with `sock.send(...)` from any of them.
    pub fn ws(&mut self, pattern: &str, handlers: WsHandlers) -> &mut Self {
        let ap = axum_pattern(pattern);
        let taken = std::mem::take(&mut self.router);
        self.router = taken.route(
            &ap,
            get(move |ws: WebSocketUpgrade| async move {
                ws.on_upgrade(move |socket| run_ws(handlers, socket))
            }),
        );
        self
    }

    /// Serve static files under a path prefix.
    /// A mount miss falls through to later mounts / the error hook.
    pub fn mount(&mut self, prefix: &str, dir: impl Into<PathBuf>) -> &mut Self {
        let prefix = prefix.trim_end_matches('/');
        self.shared.mounts.lock().unwrap().push((prefix.to_string(), dir.into()));
        self
    }

    /// Hook for requests no route or mount handled.
    pub fn on_error<F, R>(&mut self, f: F) -> &mut Self
    where
        F: Fn(Var) -> R + Send + Sync + 'static,
        R: Into<Response>,
    {
        let handler: RouteHandler = Arc::new(move |req: Var| f(req).into());
        *self.shared.error_handler.lock().unwrap() = Some(handler);
        self
    }

    // ---- lifecycle (start/stop) ----

    /// Blocking: binds and runs until [`stop()`](Self::stop). Returns an
    /// error when the bind fails instead of entering the loop; safe to call
    /// again after a stop. With a `sslCert`/`sslKey` config pair, the same
    /// port serves HTTPS.
    pub async fn start(&self) -> std::io::Result<()> {
        // clear the stale addr so `local_addr()` polling reflects THIS bind
        *self.bound.lock().unwrap() = String::new();
        let listener = tokio::net::TcpListener::bind(&self.addr).await?;
        let addr = listener.local_addr()?;
        *self.bound.lock().unwrap() = addr.to_string();
        let handle = Handle::new();
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
        *self.shutdown.lock().unwrap() = Some(stop_tx);
        let std_listener = listener.into_std()?;

        // The serve future itself, HTTP or HTTPS per the config pair.
        let serve = async {
            match &self.tls {
                Some((cert, key)) => {
                    let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
                        .await?;
                    axum_server::from_tcp_rustls(std_listener, config)
                        .handle(handle.clone())
                        .serve(self.router.clone().into_make_service())
                        .await
                }
                None => {
                    axum_server::from_tcp(std_listener)
                        .handle(handle.clone())
                        .serve(self.router.clone().into_make_service())
                        .await
                }
            }
        };

        // Shutdown on OUR signal (a watch — axum-server's own notify-based
        // signal can be missed between accept-loop iterations), then drain
        // in-flight requests by polling the connection count (also race-free)
        // and force-close whatever is left when the grace period runs out.
        let shutdown = async {
            while !*stop_rx.borrow_and_update() {
                if stop_rx.changed().await.is_err() {
                    break; // the Server was dropped: shut down with it
                }
            }
            handle.graceful_shutdown(Some(GRACE_PERIOD));
            let deadline = std::time::Instant::now() + GRACE_PERIOD;
            while handle.connection_count() > 0 && std::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            handle.shutdown();
        };

        tokio::select! {
            out = serve => out,
            _ = shutdown => Ok(()),
        }
    }

    /// Request a graceful stop of a running `start()`; safe from any thread.
    /// In-flight requests get a grace period, then the loop returns anyway —
    /// stop always makes the loop return, with a bounded worst case.
    /// Returns false when nothing is running.
    pub fn stop(&self) -> bool {
        self.shutdown
            .lock()
            .unwrap()
            .take()
            .map(|stop_tx| stop_tx.send(true).is_ok())
            .unwrap_or(false)
    }

    /// The addr the running (or last) `start()` actually bound — config port
    /// `0` resolves to the OS-assigned port.
    pub fn local_addr(&self) -> String {
        self.bound.lock().unwrap().clone()
    }
}

// ---- pattern translation (uWS-style → axum) ----

/// `/users/:id` → `/users/{id}`, trailing `/assets/*` → `/assets/{*rest}`.
/// The wildcard resurfaces in the request as `params["*"]`.
fn axum_pattern(pattern: &str) -> String {
    if pattern.is_empty() {
        return "/".to_string();
    }
    let segs: Vec<&str> = pattern.split('/').collect();
    let mut out = String::new();
    for (i, seg) in segs.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        match *seg {
            "" => {}
            s if s.starts_with(':') && s.len() > 1 => {
                out.push('{');
                out.push_str(&s[1..]);
                out.push('}');
            }
            "*" if i == segs.len() - 1 => out.push_str("{*rest}"),
            s => out.push_str(s),
        }
    }
    out
}

fn method_router(method: &str, handler: RouteHandler) -> axum::routing::MethodRouter {
    match method.to_ascii_uppercase().as_str() {
        "GET" => get(stage_req(handler)),
        "POST" => post(stage_req(handler)),
        "PUT" => put(stage_req(handler)),
        "PATCH" => patch(stage_req(handler)),
        "DELETE" => delete(stage_req(handler)),
        "OPTIONS" => options(stage_req(handler)),
        other => panic!("unsupported HTTP method {other:?}"),
    }
}

fn method_router_stream<F, Fut, R>(method: &str, f: F) -> axum::routing::MethodRouter
where
    F: Fn(Var, axum::body::Body) -> Fut + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = R> + Send + 'static,
    R: Into<Response>,
{
    match method.to_ascii_uppercase().as_str() {
        "GET" => get(stage_req_stream(f)),
        "POST" => post(stage_req_stream(f)),
        "PUT" => put(stage_req_stream(f)),
        "PATCH" => patch(stage_req_stream(f)),
        "DELETE" => delete(stage_req_stream(f)),
        "OPTIONS" => options(stage_req_stream(f)),
        other => panic!("unsupported HTTP method {other:?}"),
    }
}

/// Wrap a `Var -> Response` handler in the extractor tuple every route
/// shares. The handler runs synchronously inside the async block (handlers
/// are quick in-memory transforms; async handlers are a future step). The
/// extractor order matters (body-consuming `Bytes` last).
fn stage_req(
    handler: RouteHandler,
) -> impl Fn(Method, Uri, RawPathParams, RawQuery, HeaderMap, Bytes) -> std::future::Ready<Response>
+ Clone
+ Send
+ Sync
+ 'static {
    move |method, uri, params: RawPathParams, query: RawQuery, headers: HeaderMap, bytes: Bytes| {
        let params = params.iter().map(|(k, v)| (k.to_string(), v.to_string()));
        let req = build_req(&method, &uri, params, query.0.as_deref(), &headers, &bytes);
        std::future::ready(handler(req))
    }
}

/// The streaming flavor of [`stage_req`]: the handler is async and receives
/// the raw body stream as its second argument.
fn stage_req_stream<F, Fut, R>(
    f: F,
) -> impl Fn(Method, Uri, RawPathParams, RawQuery, HeaderMap, axum::body::Body)
    -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
+ Clone
+ Send
+ Sync
+ 'static
where
    F: Fn(Var, axum::body::Body) -> Fut + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = R> + Send + 'static,
    R: Into<Response>,
{
    move |method, uri, params: RawPathParams, query: RawQuery, headers: HeaderMap, body: axum::body::Body| {
        let f = f.clone();
        Box::pin(async move {
            let params = params.iter().map(|(k, v)| (k.to_string(), v.to_string()));
            let req = build_req(&method, &uri, params, query.0.as_deref(), &headers, &[]);
            req.set("body", Var::Null); // the raw body is the stream argument
            f(req, body).await.into()
        })
    }
}

// ---- static file mounts ----

fn serve_file(dir: &FsPath, rel: &str) -> Option<Response> {
    if rel.is_empty() {
        return None;
    }
    let full = dir.join(rel).canonicalize().ok()?;
    let root = dir.canonicalize().ok()?;
    // a resolved path outside the mount root is an escape attempt, not a file
    if !full.starts_with(&root) {
        return None;
    }
    let data = std::fs::read(&full).ok()?;
    Some(Response::bytes(data).ct(mime_of(&full)))
}

/// Compact mime table by extension — the wheel exists (`mime_guess`) but the
/// table is small and keeps the dependency tree lean.
fn mime_of(path: &FsPath) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "txt" | "md" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
    .to_string()
}

// ---- WebSocket routes (send / close) ----

enum WsOut {
    Text(String),
    Close,
}

/// A connected WebSocket client, handed to `ws()` handlers. Send stages
/// into a channel that flushes on the connection's writable moment;
/// `close()` requests a graceful close.
#[derive(Clone)]
pub struct WsSock {
    tx: mpsc::UnboundedSender<WsOut>,
}

impl WsSock {
    /// Send a text message to the client.
    pub fn send(&self, text: impl Into<String>) -> bool {
        self.tx.send(WsOut::Text(text.into())).is_ok()
    }

    /// Request a graceful close.
    pub fn close(&self) -> bool {
        self.tx.send(WsOut::Close).is_ok()
    }
}

/// The three `ws()` callbacks (`{open, message, close}`).
/// All are optional; the builder methods take plain closures.
#[derive(Clone, Default)]
pub struct WsHandlers {
    open: Option<WsOpenFn>,
    message: Option<WsMessageFn>,
    close: Option<WsCloseFn>,
}

type WsOpenFn = Arc<dyn Fn(&WsSock) + Send + Sync>;
type WsMessageFn = Arc<dyn Fn(&WsSock, Var) + Send + Sync>;
type WsCloseFn = Arc<dyn Fn(&WsSock) + Send + Sync>;

impl WsHandlers {
    pub fn new() -> Self { Self::default() }

    /// `open(sock)` — the client connected.
    pub fn on_open(mut self, f: impl Fn(&WsSock) + Send + Sync + 'static) -> Self {
        self.open = Some(Arc::new(f));
        self
    }

    /// `message(sock, data)` — one text message arrived.
    pub fn on_message(mut self, f: impl Fn(&WsSock, Var) + Send + Sync + 'static) -> Self {
        self.message = Some(Arc::new(f));
        self
    }

    /// `close(sock)` — the connection is going away.
    pub fn on_close(mut self, f: impl Fn(&WsSock) + Send + Sync + 'static) -> Self {
        self.close = Some(Arc::new(f));
        self
    }
}

async fn run_ws(handlers: WsHandlers, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<WsOut>();
    let sock = WsSock { tx };

    if let Some(open) = handlers.open.as_ref() {
        open(&sock);
    }

    // stage-and-flush: handler sends land on the connection's writable moment
    let forward = tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            let msg = match out {
                WsOut::Text(t) => Message::text(t),
                WsOut::Close => {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            };
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            Message::Text(text) => {
                if let Some(on_message) = handlers.message.as_ref() {
                    on_message(&sock, Var::from(text.to_string()));
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    if let Some(close) = handlers.close.as_ref() {
        close(&sock);
    }
    forward.abort();
}
