//! The winit window backend — gold's `windowSystemSDL` slot: a real
//! desktop window. The winit event loop runs on the calling (main) thread
//! — a macOS requirement — while the engine pump runs on any thread
//! through the ordinary [`WindowSystem`] seam: the loop forwards winit
//! events as `Var` objects into the queue the pump drains.
//!
//! ```
//! use hyg::game::{Engine, WinitWindow};
//!
//! # fn demo() -> Result<(), String> {
//! let window = WinitWindow::new("my game", (800, 450))?;
//! // spawn the engine pump on another thread with window.pump_handle(),
//! // present with WgpuRenderer::new_for_window(window.window()), then:
//! // window.run();  // blocks the main thread until quit
//! # Ok(())
//! # }
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::game::window::WindowSystem;
use crate::var::Var;

/// Internal message through the event-loop proxy.
#[derive(Debug)]
pub(crate) enum WinitMessage {
    /// The engine pump finished: close the window.
    Close,
}

struct Inner {
    /// The window, published by `resumed` (winit 0.30 creates it there).
    window: Mutex<Option<Arc<Window>>>,
    /// Events the handler mapped from winit, waiting for the pump.
    events: Mutex<Vec<Var>>,
    last_cursor: Mutex<(i64, i64)>,
    close: AtomicBool,
    proxy: Mutex<Option<EventLoopProxy<WinitMessage>>>,
    title: String,
    size: (u32, u32),
}

/// A real desktop window (gold's `windowSystem`), winit-backed.
///
/// Must be created on the main thread (macOS); `run()` blocks that thread
/// until a `quit` event (close button) or [`request_close`](Self::request_close).
/// The engine pump lives on another thread via `pump_handle()`.
pub struct WinitWindow {
    inner: Arc<Inner>,
    event_loop: Mutex<Option<EventLoop<WinitMessage>>>,
}

/// The pump-side handle: a [`WindowSystem`] fed by the winit event loop.
#[derive(Clone)]
pub struct WinitWindowHandle {
    inner: Arc<Inner>,
}

impl WinitWindow {
    /// Build the event loop and register it. The actual window is created
    /// when the loop starts (winit 0.30 creates windows in `resumed`);
    /// [`window`](Self::window) yields it as soon as it exists.
    ///
    /// # Errors
    /// The platform event loop could not be built (e.g. no display server).
    pub fn new(title: impl Into<String>, size: (u32, u32)) -> Result<Self, String> {
        let event_loop = EventLoop::<WinitMessage>::with_user_event()
            .build()
            .map_err(|e| format!("winit event loop: {e}"))?;
        let proxy = event_loop.create_proxy();
        Ok(WinitWindow {
            inner: Arc::new(Inner {
                window: Mutex::new(None),
                events: Mutex::new(Vec::new()),
                last_cursor: Mutex::new((0, 0)),
                close: AtomicBool::new(false),
                proxy: Mutex::new(Some(proxy)),
                title: title.into(),
                size,
            }),
            event_loop: Mutex::new(Some(event_loop)),
        })
    }

    /// The window once the loop has created it (`resumed`).
    pub fn window(&self) -> Option<Arc<Window>> {
        self.inner.window.lock().unwrap().clone()
    }

    /// The pump-side handle: give this to the engine thread.
    pub fn pump_handle(&self) -> WinitWindowHandle {
        WinitWindowHandle { inner: self.inner.clone() }
    }

    /// Ask the event loop to close (safe from the pump thread).
    pub fn request_close(&self) {
        self.inner.close.store(true, Ordering::SeqCst);
        if let Some(proxy) = self.inner.proxy.lock().unwrap().as_ref() {
            let _ = proxy.send_event(WinitMessage::Close);
        }
    }

    /// Run the event loop on THIS thread (main thread on macOS) until a
    /// close is requested — by the window's close button, or
    /// [`request_close`](Self::request_close) from the pump side.
    pub fn run(self) {
        let WinitWindow { inner, event_loop } = self;
        let Some(event_loop) = event_loop.into_inner().unwrap() else {
            eprintln!("winit: event loop already consumed");
            return;
        };
        let mut handler = WinitHandler { inner };
        handler.set_proxy(event_loop.create_proxy());
        let _ = event_loop.run_app(&mut handler);
    }
}

impl WinitWindowHandle {
    /// The window once the loop has created it.
    pub fn window(&self) -> Option<Arc<Window>> {
        self.inner.window.lock().unwrap().clone()
    }

    /// Ask the event loop to close (the pump's exit path).
    pub fn request_close(&self) {
        self.inner.close.store(true, Ordering::SeqCst);
        if let Some(proxy) = self.inner.proxy.lock().unwrap().as_ref() {
            let _ = proxy.send_event(WinitMessage::Close);
        }
    }
}

impl WindowSystem for WinitWindowHandle {
    fn poll(&mut self) -> Vec<Var> {
        let mut events = self.inner.events.lock().unwrap().drain(..).collect::<Vec<_>>();
        events.reverse(); // handler pushes newest-last; drain() takes newest-first
        events
    }

    fn handle_event(&mut self, _event: &Var) {}
}

struct WinitHandler {
    inner: Arc<Inner>,
}

impl WinitHandler {
    fn set_proxy(&mut self, proxy: EventLoopProxy<WinitMessage>) {
        *self.inner.proxy.lock().unwrap() = Some(proxy);
    }
}

impl ApplicationHandler<WinitMessage> for WinitHandler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attributes = Window::default_attributes()
            .with_title(self.inner.title.clone())
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.inner.size.0,
                self.inner.size.1,
            ));
        match event_loop.create_window(attributes) {
            Ok(window) => {
                *self.inner.window.lock().unwrap() = Some(Arc::new(window));
            }
            Err(e) => eprintln!("winit: window creation failed: {e}"),
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let mut last_cursor = self.inner.last_cursor.lock().unwrap();
        if let Some(event) = map_window_event(&event, &mut last_cursor) {
            self.inner.events.lock().unwrap().push(event);
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: WinitMessage) {
        if matches!(event, WinitMessage::Close) {
            self.inner.close.store(true, Ordering::SeqCst);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.inner.close.load(Ordering::SeqCst) {
            event_loop.exit();
            return;
        }
        // continuous redraw: the pump renders at its own pace, winit keeps
        // the compositor fed
        if let Some(window) = self.inner.window.lock().unwrap().as_ref() {
            window.request_redraw();
        }
    }
}

/// The engine event for a keyboard press/release (`key` lowercased —
/// characters stay as-is, named keys become `"space"`-style).
pub fn key_var(name: impl Into<String>, pressed: bool) -> Var {
    let ev = crate::object::Object::new();
    ev.set("type", Var::from("key"));
    ev.set("key", Var::from(name.into()));
    ev.set("pressed", Var::from(pressed));
    Var::from(ev)
}

/// The engine event for mouse movement.
pub fn mouse_move_var(x: i64, y: i64) -> Var {
    let ev = crate::object::Object::new();
    ev.set("type", Var::from("mouse"));
    ev.set("action", Var::from("move"));
    ev.set("x", Var::from(x));
    ev.set("y", Var::from(y));
    Var::from(ev)
}

/// The engine event for a mouse button press/release.
pub fn mouse_button_var(button: i64, pressed: bool, x: i64, y: i64) -> Var {
    let ev = crate::object::Object::new();
    ev.set("type", Var::from("mouse"));
    ev.set("action", Var::from(if pressed { "down" } else { "up" }));
    ev.set("button", Var::from(button));
    ev.set("x", Var::from(x));
    ev.set("y", Var::from(y));
    Var::from(ev)
}

/// The engine event for a wheel scroll.
pub fn wheel_var(delta: f64, x: i64, y: i64) -> Var {
    let ev = crate::object::Object::new();
    ev.set("type", Var::from("wheel"));
    ev.set("delta", Var::from(delta));
    ev.set("x", Var::from(x));
    ev.set("y", Var::from(y));
    Var::from(ev)
}

/// The engine event for a window close request.
pub fn quit_var() -> Var {
    let ev = crate::object::Object::new();
    ev.set("type", Var::from("quit"));
    Var::from(ev)
}

/// Map a winit window event to a `Var` event (`None` = nothing the engine
/// cares about).
fn map_window_event(event: &WindowEvent, last_cursor: &mut (i64, i64)) -> Option<Var> {
    match event {
        WindowEvent::KeyboardInput { event: key, .. } => {
            let pressed = key.state == ElementState::Pressed;
            let name = match &key.logical_key {
                winit::keyboard::Key::Character(c) => c.as_str().to_lowercase(),
                winit::keyboard::Key::Named(named) => format!("{named:?}").to_lowercase(),
                winit::keyboard::Key::Dead(_) => "dead".to_string(),
                winit::keyboard::Key::Unidentified(_) => return None,
            };
            Some(key_var(name, pressed))
        }
        WindowEvent::CursorMoved { position, .. } => {
            *last_cursor = (position.x as i64, position.y as i64);
            Some(mouse_move_var(position.x as i64, position.y as i64))
        }
        WindowEvent::MouseInput { state, button, .. } => {
            let (x, y) = *last_cursor;
            let button = match button {
                MouseButton::Left => 0,
                MouseButton::Right => 1,
                MouseButton::Middle => 2,
                MouseButton::Back => 3,
                MouseButton::Forward => 4,
                MouseButton::Other(n) => *n as i64,
            };
            Some(mouse_button_var(button, *state == ElementState::Pressed, x, y))
        }
        WindowEvent::MouseWheel { delta, .. } => {
            let (x, y) = *last_cursor;
            let amount = match delta {
                MouseScrollDelta::LineDelta(_, dy) => *dy as f64,
                MouseScrollDelta::PixelDelta(p) => p.y,
            };
            Some(wheel_var(amount, x, y))
        }
        WindowEvent::CloseRequested => Some(quit_var()),
        _ => None,
    }
}
