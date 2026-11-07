//! The streaming window backend: the headless render buffer becomes a web
//! page. A tiny axum server serves a canvas, pushes the latest frame as a
//! binary WebSocket message, and turns browser events (keys, mouse, the
//! page's Quit button) back into `Var` events — the engine can't tell it
//! apart from a real windowing system. The page auto-opens in the browser
//! on launch, which also makes the whole loop trivially inspectable: an AI
//! (or a test) can read the frame, click, and send keys without a GUI.
//!
//! The surface is mobile-ready and hand-off capable: fit mode upscales
//! bilinearly and re-fits on rotation, a toolbox menu (☿) carries the
//! controls hints plus a `/qr` SVG encoding the LAN-reachable URL (open the
//! stream on another device), and the desktop fullscreen button hands the
//! frames off to the **native view** (`native_view::NativeView` — the same
//! buffer, a second consumer; the pump never restarts). An opt-in
//! [`StreamingWindow::with_token`] gates the page and the WS upgrade behind
//! a random hex `?t=` token.
//!
//! Behind the `stream` feature (reuses axum + tokio from the web layer).

use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};

use crate::game::render::FrameSink;
use crate::game::window::WindowSystem;
use crate::var::Var;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};

const PAGE: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no, viewport-fit=cover">
<title>☿ Hydrargyros — streaming window</title>
<style>
body { margin: 0; background: #14161a; color: #e8e6e3; font-family: system-ui, sans-serif;
       display: flex; flex-direction: column; align-items: center; gap: .75rem;
       padding: 1rem; height: 100vh; height: 100dvh; box-sizing: border-box;
       touch-action: manipulation; }
/* fit upscaling is BILINEAR (the browser default): `pixelated` would make
   every non-integer fit scale nearest-neighbor crunchy — `auto` keeps the
   stream smooth at any scale and changes nothing at 1:1 */
canvas { image-rendering: auto; border: 1px solid #2a2e36; border-radius: 8px;
         max-width: 96vw; max-height: 78vh; background: #000;
         touch-action: none; /* the canvas is a game surface: no double-tap zoom, no scroll */ }
#glyph { margin: 0; font-size: 2rem; color: #dce0e8; font-weight: 400; }
/* object-fit: contain keeps the CSS fallback letterboxed too when the
   max-height clamp would otherwise distort the aspect */
body.fit canvas { width: 96vw; height: auto; max-height: 82vh; object-fit: contain; }
.row { display: flex; align-items: center; gap: 1.25rem; flex-wrap: wrap; justify-content: center; }
button { background: #4f6df5; color: #fff; border: 0; border-radius: 6px;
         padding: .55rem 1.1rem; font-size: 1rem; min-height: 2.75rem; }
button.secondary { background: #2a2e36; color: #e8e6e3; }
#status { color: #9aa3b2; font-size: .85rem; margin: 0 1.5rem; min-width: 6rem; }
#toolbox { position: fixed; top: .75rem; right: .75rem; z-index: 10; }
#toolbox-btn { width: 2.75rem; padding: 0; font-size: 1.25rem; }
#menu { position: absolute; top: 3.3rem; right: 0; width: min(20rem, 86vw);
        background: #1c2027; border: 1px solid #2a2e36; border-radius: 10px;
        padding: .9rem 1rem; box-shadow: 0 .5rem 1.5rem rgb(0 0 0 / .5); }
#menu h3 { margin: .2rem 0 .4rem; font-size: .8rem; text-transform: uppercase;
           letter-spacing: .08em; color: #9aa3b2; }
#menu ul { margin: 0 0 .8rem; padding-left: 1.1rem; font-size: .9rem; line-height: 1.5; }
#menu img { display: block; width: min(11rem, 60vw); margin: 0 auto .5rem;
            background: #fff; border-radius: 6px; padding: .3rem; box-sizing: border-box; }
#handoff { display: block; font-size: .72rem; color: #b8c0cc; word-break: break-all;
           user-select: all; cursor: text; }
#menu[hidden] { display: none; }
</style></head>
<body>
<h1 id="glyph" title="hydrargyros — Hg">☿</h1>
<canvas id="screen" width="320" height="180"></canvas>
<div class="row">
  <button id="quit">quit</button>
  <button id="fullscreen" class="secondary">fullscreen</button>
  <button id="fit" class="secondary">fit</button>
  <span id="status">connecting…</span>
</div>
<div id="toolbox">
  <button id="toolbox-btn" class="secondary" aria-label="toolbox menu" aria-expanded="false">☿</button>
  <div id="menu" hidden>
    <h3>controls</h3>
    <ul>
      <li>left-drag — orbit</li>
      <li>wheel — zoom</li>
      <li>keys go to the engine (the sandbox: space flips gravity, r resets)</li>
    </ul>
    <h3>open on another device</h3>
    <img id="qr" src="/qr" alt="QR code linking to this stream">
    <code id="handoff">__HANDOFF_URL__</code>
  </div>
</div>
<script>
const canvas = document.getElementById("screen");
const ctx = canvas.getContext("2d");
const status = document.getElementById("status");
// the access token (if the stream has one) rides the page URL — pass the
// same query on to the socket
const ws = new WebSocket(`ws://${location.host}/ws${location.search}`);
ws.binaryType = "arraybuffer";
ws.onopen = () => { status.textContent = "streaming"; };
ws.onclose = () => { status.textContent = "disconnected"; };
ws.onmessage = (m) => {
  if (m.data instanceof ArrayBuffer) {
    const dv = new DataView(m.data);
    const w = dv.getUint32(0, true), h = dv.getUint32(4, true);
    if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; refit(); }
    const pixels = new Uint8ClampedArray(m.data, 8, w * h * 4);
    ctx.putImageData(new ImageData(pixels, w, h), 0, 0);
  } else {
    status.textContent = m.data;
  }
};
function send(ev) { if (ws.readyState === 1) ws.send(JSON.stringify(ev)); }
addEventListener("keydown", (e) => send({ type: "key", key: e.key, pressed: true }));
addEventListener("keyup",   (e) => send({ type: "key", key: e.key, pressed: false }));
function canvasPos(e) {
  const r = canvas.getBoundingClientRect();
  return { x: Math.round((e.clientX - r.left) / r.width * canvas.width),
           y: Math.round((e.clientY - r.top) / r.height * canvas.height) };
}
canvas.addEventListener("mousedown", (e) => { const p = canvasPos(e); send({ type: "mouse", action: "down", button: e.button, x: p.x, y: p.y }); });
canvas.addEventListener("mouseup",   (e) => { const p = canvasPos(e); send({ type: "mouse", action: "up", button: e.button, x: p.x, y: p.y }); });
canvas.addEventListener("mousemove", (e) => { const p = canvasPos(e); send({ type: "mouse", action: "move", x: p.x, y: p.y }); });
canvas.addEventListener("wheel",     (e) => { const p = canvasPos(e); send({ type: "wheel", delta: e.deltaY, x: p.x, y: p.y }); e.preventDefault(); }, { passive: false });
// phones/tablets (pointer: coarse + a mobile UA): fullscreen does DEVICE
// fullscreen — requestFullscreen + an orientation lock, both best-effort
// (refusals never break the page). Desktop: hand the stream off to the
// NATIVE window — the same frames from the same buffer, no app restart
// (the app spawns the view; this page keeps streaming until closed).
const isMobile = matchMedia("(pointer: coarse)").matches &&
                 /Android|iPhone|iPad|iPod|Mobile|Tablet/i.test(navigator.userAgent);
document.getElementById("quit").addEventListener("click", () => send({ type: "quit" }));
document.getElementById("fullscreen").addEventListener("click", async () => {
  if (isMobile) {
    try {
      if (document.fullscreenElement) {
        await document.exitFullscreen();
        if (screen.orientation && screen.orientation.unlock) { screen.orientation.unlock(); }
      } else if (document.documentElement.requestFullscreen) {
        await document.documentElement.requestFullscreen();
        if (screen.orientation && screen.orientation.lock) {
          screen.orientation.lock("landscape").catch(() => {});
        }
      }
    } catch (e) { /* refused (iOS etc.) — the page keeps working */ }
    return;
  }
  if (document.fullscreenElement) { document.exitFullscreen(); }
  send({ type: "native_fullscreen" });
});
// fit: explicit letterbox math (aspect preserved, landscape or portrait),
// recomputed on rotation and viewport changes so a rotated phone keeps the
// canvas fitted
let fitMode = false;
const fitBtn = document.getElementById("fit");
function refit() {
  if (!fitMode) { return; }
  const vw = visualViewport ? visualViewport.width : innerWidth;
  const vh = visualViewport ? visualViewport.height : innerHeight;
  const scale = Math.min((vw * 0.96) / canvas.width, (vh * 0.82) / canvas.height);
  canvas.style.width = Math.floor(canvas.width * scale) + "px";
  canvas.style.height = Math.floor(canvas.height * scale) + "px";
}
fitBtn.addEventListener("click", () => {
  fitMode = !fitMode;
  document.body.classList.toggle("fit", fitMode);
  if (fitMode) { refit(); } else { canvas.style.width = ""; canvas.style.height = ""; }
  fitBtn.textContent = fitMode ? "fit: on" : "fit";
});
addEventListener("orientationchange", refit);
if (visualViewport) { visualViewport.addEventListener("resize", refit); }
addEventListener("resize", refit);
// the toolbox menu: controls hints + the QR hand-off
const menu = document.getElementById("menu");
const toolboxBtn = document.getElementById("toolbox-btn");
toolboxBtn.addEventListener("click", () => {
  menu.hidden = !menu.hidden;
  toolboxBtn.setAttribute("aria-expanded", menu.hidden ? "false" : "true");
});
</script>
</body></html>"#;

/// Per-server state shared by the routes: the frame source, the event pipe,
/// and the access config (the opt-in token + the LAN URL for `/qr`).
#[derive(Clone)]
struct ServerState(Arc<InnerState>);

struct InnerState {
    sink: FrameSink,
    events: mpsc::Sender<Var>,
    /// `Some(token)` → the page and the WS upgrade require `?t=<token>`.
    /// Armed by [`StreamingWindow::with_token`] (which runs right after the
    /// server thread starts — connections accepted before arming are served
    /// without the check, so arm immediately; documented on `with_token`).
    token: Mutex<Option<String>>,
    /// The LAN-reachable page URL base (no token) for the QR hand-off.
    lan_base: String,
}

impl InnerState {
    /// The URL the QR hand-off advertises: the LAN base, `?t=` appended
    /// when a token is armed.
    fn handoff_url(&self) -> String {
        match &*self.token.lock().unwrap() {
            Some(token) => format!("{}?t={token}", self.lan_base),
            None => self.lan_base.clone(),
        }
    }

    /// `Some(403 page)` when a token is armed and `query` doesn't carry it;
    /// `None` lets the request through. A plain string compare is fine for
    /// a LAN prototyping gate.
    fn check_token(&self, query: &HashMap<String, String>) -> Option<Response> {
        let token = self.token.lock().unwrap().clone()?;
        if query.get("t").is_some_and(|t| *t == token) {
            return None;
        }
        Some(
            (
                StatusCode::FORBIDDEN,
                Html(
                    "<!DOCTYPE html><html><body><p>403 — this stream needs its access token; \
                 open the URL the app printed.</p></body></html>",
                ),
            )
                .into_response(),
        )
    }
}

/// The streaming window backend: serves the page, pushes frames from a
/// [`FrameSink`], and delivers browser events through
/// [`WindowSystem::poll`](crate::game::WindowSystem::poll) — keys, mouse,
/// wheel, and a page `quit` button, all as `Var` objects.
pub struct StreamingWindow {
    url: String,
    base_url: String,
    access: Arc<InnerState>,
    events: Mutex<mpsc::Receiver<Var>>,
    event_tx: mpsc::Sender<Var>,
    _server: std::thread::JoinHandle<()>,
}

impl StreamingWindow {
    /// Bind an HTTP server on `bind` (e.g. `"127.0.0.1:8400"` — port `0`
    /// picks a free one), streaming frames from `sink`. With
    /// `open_browser`, launches the system browser at the page.
    ///
    /// # Errors
    /// The bind failed.
    pub fn new(bind: &str, sink: FrameSink, open_browser: bool) -> Result<Self, String> {
        let (event_tx, event_rx) = mpsc::channel::<Var>();
        let server_tx = event_tx.clone();
        let std_listener =
            std::net::TcpListener::bind(bind).map_err(|e| format!("stream bind {bind}: {e}"))?;
        let addr = std_listener
            .local_addr()
            .map_err(|e| format!("stream local_addr: {e}"))?;
        let base_url = format!("http://{addr}/");
        let access = Arc::new(InnerState {
            sink,
            events: server_tx,
            token: Mutex::new(None),
            lan_base: format!("http://{}:{}/", lan_ip(bind), addr.port()),
        });
        let state = ServerState(Arc::clone(&access));

        let handle = std::thread::Builder::new()
            .name("hydrargyros-stream".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("stream runtime");
                // tokio's into_std left the fd blocking; from_std needs the opposite
                std_listener
                    .set_nonblocking(true)
                    .expect("stream nonblocking");
                runtime.block_on(async move {
                    let app = Router::new()
                        .route("/", get(page))
                        .route("/ws", get(ws_upgrade))
                        .route("/qr", get(qr))
                        .with_state(state);
                    let listener =
                        tokio::net::TcpListener::from_std(std_listener).expect("stream listener");
                    axum::serve(listener, app).await.expect("stream server");
                });
            })
            .map_err(|e| format!("stream thread: {e}"))?;

        if open_browser {
            open_in_browser(&base_url);
        }
        Ok(StreamingWindow {
            url: base_url.clone(),
            base_url,
            access,
            events: Mutex::new(event_rx),
            event_tx,
            _server: handle,
        })
    }

    /// Require a random hex token on the page URL and the WS upgrade (the
    /// opt-in gate — off by default, zero behavior change without it).
    /// [`url`](Self::url) becomes the tokenized URL and the `/qr` hand-off
    /// embeds it.
    ///
    /// The server thread is already accepting by the time this runs (the
    /// builder rides the returned handle), so arm immediately after
    /// `new()` — a connection accepted inside that gap is served without
    /// the check. If the page auto-opened, re-open [`url`](Self::url): the
    /// browser launched before the token existed.
    pub fn with_token(mut self) -> Self {
        let token = random_token();
        *self.access.token.lock().unwrap() = Some(token.clone());
        self.url = format!("{}?t={token}", self.base_url);
        self
    }

    /// The page URL (useful when the browser wasn't auto-opened) — with
    /// `?t=<token>` appended once [`with_token`](Self::with_token) armed
    /// the gate.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Non-blocking drain of browser events — the `&self` form, callable
    /// through a shared handle. ([`WindowSystem::poll`] delegates here.)
    pub fn drain(&self) -> Vec<Var> {
        let receiver = self.events.lock().unwrap();
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }
        events
    }

    /// Inject an event as if the browser had sent it — the seam a system
    /// tray (or a test) uses to reach the engine.
    pub fn inject(&self, event: Var) {
        let _ = self.event_tx.send(event);
    }
}

impl WindowSystem for StreamingWindow {
    /// Non-blocking: drains everything the browser sent since last poll.
    fn poll(&mut self) -> Vec<Var> {
        self.drain()
    }

    fn handle_event(&mut self, _event: &Var) {}
}

/// The shared-handle form drives the pump from another thread.
impl WindowSystem for std::sync::Arc<StreamingWindow> {
    fn poll(&mut self) -> Vec<Var> {
        self.drain()
    }

    fn handle_event(&mut self, _event: &Var) {}
}

/// The page route: token-gated when armed, with the hand-off URL injected
/// for the toolbox's "open on another device" text.
async fn page(
    State(state): State<ServerState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if let Some(forbidden) = state.0.check_token(&query) {
        return forbidden;
    }
    Html(PAGE.replace("__HANDOFF_URL__", &state.0.handoff_url())).into_response()
}

/// The WebSocket upgrade route: state carries the sink and the event pipe.
/// Validates the token when armed (same as the page).
async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<ServerState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if let Some(forbidden) = state.0.check_token(&query) {
        return forbidden;
    }
    let (sink, events) = (state.0.sink.clone(), state.0.events.clone());
    ws.on_upgrade(move |socket| stream_socket(socket, sink, events))
        .into_response()
}

/// The QR hand-off route: an SVG encoding the LAN-reachable page URL (the
/// token included when armed). `/qr` stays open by design — it IS the
/// hand-off; on a tokenized stream it's only as secret as your LAN. The
/// URL uses the LAN IP (the UDP-connect trick below), never 127.0.0.1 — a
/// local-only bind keeps the QR harmless (other devices can't connect).
async fn qr(State(state): State<ServerState>) -> Response {
    match qr_svg(&state.0.handoff_url()) {
        Ok(svg) => ([(header::CONTENT_TYPE, "image/svg+xml")], svg).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("qr: {e}")).into_response(),
    }
}

/// One connected browser: push frames when they change, receive events.
async fn stream_socket(socket: WebSocket, sink: FrameSink, events: mpsc::Sender<Var>) {
    let (mut sender, mut receiver) = socket.split();
    let mut generation = 0u64;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(33));
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if let Some(frame) = sink.latest_since(generation) {
                    generation = frame.generation;
                    let mut message =
                        Vec::with_capacity(8 + frame.rgba.len());
                    message.extend_from_slice(&frame.width.to_le_bytes());
                    message.extend_from_slice(&frame.height.to_le_bytes());
                    message.extend_from_slice(&frame.rgba);
                    if sender.send(Message::Binary(message.into())).await.is_err() {
                        break;
                    }
                }
            }
            incoming = receiver.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        // browser event JSON → a Var event for the engine
                        if let Ok(event) = serde_json::from_str::<Var>(&text) {
                            if events.send(event).is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

/// The QR as a standalone SVG: qrcodegen 1.8 generates the module matrix
/// (its `to_svg_string` lives in the demo, not the library), the shape is
/// ours — one `<rect>`-path square per dark module over a white field.
fn qr_svg(text: &str) -> Result<String, String> {
    let qr = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium)
        .map_err(|e| format!("qr encode: {e}"))?;
    let n = qr.size();
    let mut path = String::new();
    for y in 0..n {
        for x in 0..n {
            if qr.get_module(x, y) {
                path.push_str(&format!("M{x} {y}h1v1h-1z"));
            }
        }
    }
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {n} {n}\" \
         shape-rendering=\"crispEdges\"><rect width=\"{n}\" height=\"{n}\" fill=\"#fff\"/>\
         <path d=\"{path}\" fill=\"#14161a\"/></svg>"
    ))
}

/// The LAN-reachable IP for the QR hand-off: a UDP `connect` to a public
/// address routes the socket WITHOUT sending a packet, and `local_addr`
/// then names the interface a real connection would use — the LAN IP, not
/// 127.0.0.1. Falls back to the configured bind host when there's no route
/// (a loopback bind keeps the QR harmless: nothing reachable to open).
fn lan_ip(bind: &str) -> String {
    if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("8.8.8.8:80").is_ok() {
            if let Ok(local) = sock.local_addr() {
                if !local.ip().is_loopback() {
                    return local.ip().to_string();
                }
            }
        }
    }
    match bind.rsplit_once(':') {
        Some((host, _)) => host.trim_matches(['[', ']']).to_string(),
        None => bind.to_string(),
    }
}

/// A random hex token: 16 bytes from the OS (`/dev/urandom`, stdlib only).
/// Where that device doesn't exist, a best-effort fallback mixes the pid,
/// the clock, and `RandomState`'s per-process random seed through
/// `DefaultHasher` — fine for a LAN prototyping gate.
fn random_token() -> String {
    use std::hash::{Hash, Hasher};
    let mut bytes = [0u8; 16];
    let from_os = std::fs::File::open("/dev/urandom")
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut bytes))
        .is_ok();
    if !from_os {
        let mut first = std::collections::hash_map::DefaultHasher::new();
        std::process::id().hash(&mut first);
        std::time::SystemTime::now().hash(&mut first);
        // the hasher's stack address differs per run — cheap extra entropy
        (&first as *const _ as usize).hash(&mut first);
        let hi = first.finish();
        let mut second = std::collections::hash_map::DefaultHasher::new();
        std::thread::current().id().hash(&mut second);
        hi.hash(&mut second);
        bytes[..8].copy_from_slice(&hi.to_le_bytes());
        bytes[8..].copy_from_slice(&second.finish().to_le_bytes());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn open_in_browser(url: &str) {
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", url])
        .spawn();
}
