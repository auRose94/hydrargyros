//! Window backends — the registered-backend pattern: Rust traits and a
//! register/create registry. Events cross the boundary as `Var` objects
//! (`{ "type": "quit" }`, `{ "type": "key", "key": "w" }`, …); the headless
//! backend runs the whole loop in tests without a GPU or window.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::{Arc, OnceLock, RwLock};

use crate::object::Object;
use crate::var::Var;

/// A window/display backend: the engine polls it for events each frame and
/// hands events back for its own handling.
pub trait WindowSystem: Send {
    /// All events queued since the last poll, as `Var` objects.
    fn poll(&mut self) -> Vec<Var>;

    /// The window's own reaction to an event (the engine forwards every
    /// polled event here, after checking for `"quit"` itself).
    fn handle_event(&mut self, event: &Var);
}

/// Factory shape for the backend registry: config `Var` in, backend out.
pub type WindowFactory =
    Arc<dyn Fn(&Var) -> Result<Box<dyn WindowSystem>, String> + Send + Sync>;

fn backends() -> &'static RwLock<HashMap<String, WindowFactory>> {
    static BACKENDS: OnceLock<RwLock<HashMap<String, WindowFactory>>> = OnceLock::new();
    BACKENDS.get_or_init(|| {
        let mut map = HashMap::new();
        map.insert("headless".to_string(), headless_factory());
        RwLock::new(map)
    })
}

fn headless_factory() -> WindowFactory {
    Arc::new(|_config| Ok(Box::new(HeadlessWindow::new())))
}

/// Register a window backend under a name (replaces any existing one).
pub fn register_window_backend(name: &str, factory: WindowFactory) {
    backends().write().unwrap().insert(name.to_string(), factory);
}

/// Create a window backend by name (looked up in the registry).
///
/// # Errors
/// Unknown backend name, or the backend's own factory failure.
pub fn create_window_backend(name: &str, config: &Var) -> Result<Box<dyn WindowSystem>, String> {
    let factory = backends()
        .read()
        .unwrap()
        .get(name)
        .cloned()
        .ok_or_else(|| format!("unknown window backend {name:?}"))?;
    factory(config)
}

/// The no-window backend: no events unless scripted, and an automatic
/// `quit` once the scripted ones run out, so `Engine::start` terminates.
/// Frame behavior is fully deterministic.
pub struct HeadlessWindow {
    events: VecDeque<Var>,
    quit_on_empty: bool,
    quit_sent: bool,
    /// Silent-frames countdown for [`for_frames`](Self::for_frames).
    frames_left: Option<usize>,
}

impl HeadlessWindow {
    /// A window that never emits events and never quits — pair with
    /// `Engine::stop` from a component, or drive frames with
    /// [`Engine::update`](crate::game::Engine::update) directly.
    pub fn new() -> Self {
        HeadlessWindow {
            events: VecDeque::new(),
            quit_on_empty: false,
            quit_sent: false,
            frames_left: None,
        }
    }

    /// A scripted window: emits the given events (all on the first poll),
    /// then a `quit` event on the next poll so `Engine::start` terminates.
    pub fn with_events(events: impl IntoIterator<Item = Var>) -> Self {
        HeadlessWindow {
            events: events.into_iter().collect(),
            quit_on_empty: true,
            quit_sent: false,
            frames_left: None,
        }
    }

    /// A window that stays silent for exactly `frames` frames, then quits —
    /// the headless way to run a simulation for a known amount of time.
    pub fn for_frames(frames: usize) -> Self {
        HeadlessWindow {
            events: VecDeque::new(),
            quit_on_empty: false,
            quit_sent: false,
            frames_left: Some(frames),
        }
    }
}

impl Default for HeadlessWindow {
    fn default() -> Self { Self::new() }
}

impl WindowSystem for HeadlessWindow {
    fn poll(&mut self) -> Vec<Var> {
        if let Some(left) = &mut self.frames_left {
            if *left > 0 {
                *left -= 1;
                return Vec::new();
            }
        }
        if !self.events.is_empty() {
            return self.events.drain(..).collect();
        }
        if (self.quit_on_empty || self.frames_left.is_some()) && !self.quit_sent {
            self.quit_sent = true;
            let ev = Object::new();
            ev.set("type", Var::from("quit"));
            return vec![Var::from(ev)];
        }
        Vec::new()
    }

    fn handle_event(&mut self, _event: &Var) {}
}
