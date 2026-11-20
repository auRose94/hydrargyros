//! The Studio shell — a winit window with the engine pump on another
//! thread, the engine's frame blitted under egui panels on ONE surface.
//! The layering the design's open question asked to verify, as code:
//! winit 0.30 events feed egui-winit, the engine frame is the underlay
//! pass (the native view's blit, shared), and egui composites on top in
//! a second load pass of the same surface view.
//!
//! M2 adds the manipulation layer on that ground: a transparent
//! CentralPanel over the letterboxed viewport (the pick/gizmo surface),
//! screen-space transform gizmos (the math in
//! [`super::gizmo`](super::gizmo), feature-free), viewport picking
//! ([`super::pick`](super::pick)), an undo/redo stack with per-drag
//! coalescing (Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z, Del deletes), scene
//! save/load through the M0 carrier, and the Studio theme (dark default,
//! quicksilver — [`super::style`]).
//!
//! Threading mirrors the native view's shape, roles flipped: the editor is
//! the app, so winit owns the MAIN thread (the pump is the portable worker
//! half), and the pump drives the [`Engine`](crate::game::Engine) through
//! a shared mutex. The panels read/write the live tree under that lock
//! between pump frames — a value drag can cost the pump a frame;
//! correctness first. And per the debugger ruling: a paused graph stops
//! ADVANCING, this loop keeps DRAWING — the editor never shows a dead
//! window.
//!
//! Units, pinned once: the editor's math ([`super::pick`]/
//! [`super::gizmo`]) runs in PHYSICAL pixels; egui runs in POINTS. Every
//! crossing converts by `pixels_per_point` (pointer in → ×ppp, paint
//! out → ÷ppp).
//!
//! Headless-first: none of this runs in tests (windowing is exercised by
//! hand); the testable halves are [`super::ui`], [`super::pick`],
//! [`super::gizmo`], and the scene-io ops they drive.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::carrier;
use crate::{a, o, Var};
use crate::game::editor::gizmo::{self, GizmoHit, GizmoLayout, GizmoMode};
use crate::game::editor::pick::{self, Viewport};
use crate::game::editor::style;
use crate::game::editor::ui::EditorUi;
use crate::game::engine::Engine;
use crate::game::entity::Entity;
use crate::game::render::FrameSink;
use crate::game::scene::Scene;
use crate::game::camera::Projection;
use crate::var::{Quat, Vec3};

/// Launch the Studio shell: spawn the engine pump on a worker thread and
/// run the winit event loop on THIS thread (winit's main-thread rule —
/// the pump is the portable half). Blocks until the window closes.
///
/// The engine must already carry its offscreen renderer with `sink`
/// attached ([`WgpuRenderer::attach_sink`](crate::game::WgpuRenderer::attach_sink))
/// — the shell is a second consumer of the frames, exactly the native
/// view's contract; the pump loops `update → draw → render` until the
/// window closes.
///
/// # Errors
/// No display server, no wgpu adapter for the surface, or the pump thread
/// could not spawn.
pub fn launch(engine: Engine, sink: FrameSink, title: impl Into<String>) -> Result<(), String> {
    let engine = Arc::new(Mutex::new(engine));
    let pumping = Arc::new(AtomicBool::new(true));
    // EDIT MODE: paused by default — the sim does not run until Play, so
    // edits (gizmo drags, inspector writes) stick instead of being
    // reverted by the physics sync a frame later. Paused still draws +
    // renders: the viewport is live, only the update pass is gated.
    let playing = Arc::new(AtomicBool::new(false));
    // scene-dirty gate: while paused, the pump only re-renders when an
    // edit landed (a paused, clean scene re-uploading + reading back the
    // offscreen frame every 16ms starves the UI thread of the engine
    // lock — the sluggish-resize feel)
    let dirty = Arc::new(AtomicBool::new(true));
    // the viewport panel's size — the pump renders the offscreen frame
    // AT this size, so the frame fills the panel with no letterbox bars
    let frame_size = Arc::new(Mutex::new([960u32, 540]));
    // the camera-preview camera (Some = render the preview each tick)
    let pip_camera = Arc::new(Mutex::new(None));
    let console = Arc::new(Mutex::new(Vec::new()));
    let ai_busy = Arc::new(AtomicBool::new(false));
    let error_beam = Arc::new(AtomicBool::new(false));
    let pump = {
        let engine = Arc::clone(&engine);
        let pumping = Arc::clone(&pumping);
        let playing = Arc::clone(&playing);
        let dirty = Arc::clone(&dirty);
        let frame_size = Arc::clone(&frame_size);
        let pip_camera = Arc::clone(&pip_camera);
        let console = Arc::clone(&console);
        let error_beam = Arc::clone(&error_beam);
        std::thread::Builder::new()
            .name("hyg-studio-pump".to_string())
            .spawn(move || {
                let mut rendered_size = [960u32, 540];
                while pumping.load(Ordering::SeqCst) {
                    let started = Instant::now();
                    let run_frame =
                        playing.load(Ordering::SeqCst) || dirty.swap(false, Ordering::SeqCst);
                    if run_frame {
                        // poison-tolerant lock: a script panic mid-update
                        // poisons the mutex; the editor keeps living
                        let mut engine = match engine.lock() {
                            Ok(guard) => guard,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        // resize the offscreen frame to the panel
                        let size = *frame_size.lock().unwrap();
                        if size != rendered_size {
                            if let Some(renderer) = engine.renderer_mut() {
                                renderer.set_size(size[0], size[1]);
                            }
                            rendered_size = size;
                        }
                        let was_playing = playing.load(Ordering::SeqCst);
                        // a script throw panics through the component —
                        // CAUGHT here: the console shows the error, the
                        // sim stops (edit mode), the editor stays alive
                        let frame_result = std::panic::catch_unwind(
                            std::panic::AssertUnwindSafe(|| {
                                if was_playing {
                                    engine.update(1.0 / 60.0);
                                }
                                engine.draw();
                                engine.render();
                            }),
                        );
                        if let Err(panic) = frame_result {
                            let message = panic
                                .downcast_ref::<String>()
                                .cloned()
                                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                                .unwrap_or_else(|| "unknown script panic".to_string());
                            console
                                .lock()
                                .unwrap()
                                .push(format!("SCRIPT PANIC: {message}"));
                            playing.store(false, Ordering::SeqCst);
                            // the beam turns HARD RED: an error is an
                            // alarm, it sticks until the session clears it
                            error_beam.store(true, Ordering::SeqCst);
                        }
                        // the camera preview: render the pip camera's
                        // view into tag 1 (tags keep the slots apart)
                        let pip = *pip_camera.lock().unwrap();
                        if let Some(pip_camera) = pip {
                            if let Some(renderer) = engine.renderer_mut() {
                                renderer.set_frame_tag(1);
                            }
                            engine.set_camera(pip_camera);
                            engine.draw();
                            engine.render();
                            if let Some(renderer) = engine.renderer_mut() {
                                renderer.set_frame_tag(0);
                            }
                        }
                    }
                    // pace to ~60 fps of pump work; the display loop
                    // presents at its own cadence
                    let spent = started.elapsed();
                    std::thread::sleep(Duration::from_millis(16).saturating_sub(spent));
                }
            })
            .map_err(|e| format!("studio pump thread: {e}"))?
    };

    let result = run_shell(
        engine,
        sink,
        title.into(),
        playing,
        dirty,
        frame_size,
        pip_camera,
        console,
        ai_busy,
        error_beam,
    );
    pumping.store(false, Ordering::SeqCst);
    let _ = pump.join();
    result
}

/// The winit event loop + egui + blit, on the current thread.
#[allow(clippy::too_many_arguments)]
fn run_shell(
    engine: Arc<Mutex<Engine>>,
    sink: FrameSink,
    title: String,
    playing: Arc<AtomicBool>,
    dirty: Arc<AtomicBool>,
    frame_size: Arc<Mutex<[u32; 2]>>,
    pip_camera: Arc<Mutex<Option<crate::game::Camera>>>,
    console: Arc<Mutex<Vec<String>>>,
    ai_busy: Arc<AtomicBool>,
    error_beam: Arc<AtomicBool>,
) -> Result<(), String> {
    let mut builder = winit::event_loop::EventLoop::<ShellMessage>::with_user_event();
    // the editor normally owns the main thread; the escape hatches cost
    // nothing and keep an embedded launch (a view spawned off-main) viable
    #[cfg(target_os = "linux")]
    {
        // scoped trait imports — both traits define `with_any_thread`, and
        // only the active backend reads its own flag (the native view's
        // pattern)
        {
            use winit::platform::x11::EventLoopBuilderExtX11;
            builder.with_any_thread(true);
        }
        {
            use winit::platform::wayland::EventLoopBuilderExtWayland;
            builder.with_any_thread(true);
        }
    }
    let event_loop = builder
        .build()
        .map_err(|e| format!("studio: event loop: {e} (no display server?)"))?;
    let scene_camera = engine.lock().map(|engine| *engine.camera()).unwrap_or_default();
    let quit = Arc::new(AtomicBool::new(false));
    register_builtin_component_editors();
    let mut handler = ShellHandler {
        engine,
        sink,
        title,
        playing,
        dirty,
        quit,
        frame_size,
        pip_camera,
        console,
        ai_busy,
        error_beam,
        state: EditorState::new(scene_camera),
        window: None,
        view: None,
        egui_ctx: None,
        egui_state: None,
    };
    // EDITOR_SELECT=<name>: pre-select the first entity bearing that name
    // (depth-first through the roots) — a deterministic hook for review
    // screenshots and pixel probes; unset, the studio starts unselected
    if let Ok(want) = std::env::var("EDITOR_SELECT") {
        let engine = crate::lock(&handler.engine);
        let mut found = None;
        for (i, root) in engine.entities().iter().enumerate() {
            let mut path = vec![i];
            if root.name() == want || find_by_name(root, &want, &mut path) {
                found = Some(path);
                break;
            }
        }
        drop(engine);
        if let Some(path) = found {
            handler.state.ui.select(path);
        }
    }
    // EDITOR_PLAY=1: start in play mode (the deterministic-screenshot
    // hook set's third member — also the rainbow beam's processing
    // state, since playing counts as processing)
    if std::env::var("EDITOR_PLAY").as_deref() == Ok("1") {
        handler.playing.store(true, Ordering::SeqCst);
    }
    // EDITOR_DOCK=<tab>: open a bottom-dock tab at launch (physics /
    // audio / log / stats / console / debugger / profiler) — the
    // screenshot hook's other half; unset keeps the Log tab
    match std::env::var("EDITOR_DOCK").ok().as_deref() {
        Some("physics") => handler.state.dock_tab = DockTab::Physics,
        Some("audio") => handler.state.dock_tab = DockTab::Audio,
        Some("stats") => handler.state.dock_tab = DockTab::Stats,
        Some("console") => handler.state.dock_tab = DockTab::Console,
        Some("debugger") => handler.state.dock_tab = DockTab::Debugger,
        Some("profiler") => handler.state.dock_tab = DockTab::Profiler,
        Some("log") | None => {}
        Some(other) => eprintln!("studio: unknown EDITOR_DOCK {other:?} (log kept)"),
    }
    event_loop
        .run_app(&mut handler)
        .map_err(|e| format!("studio: event loop error: {e}"))
}

impl EditorState {
    /// Switch the beam's TARGET mode: the paint ramps from the
    /// previous target over BEAM_TRANSITION_SECONDS. An explicit
    /// switch cancels any timed flash.
    fn set_beam(&mut self, mode: style::BeamMode) {
        if self.beam_mode != mode {
            self.beam_prev = self.beam_mode;
            self.beam_mode = mode;
            self.beam_blend = 0.0;
        }
        self.beam_flash = None;
    }

    /// A TIMED flash: the mode takes over, then the beam returns to
    /// `beam_base` (Saved hums green, then the rainbow returns).
    fn flash_beam(&mut self, mode: style::BeamMode, hold: std::time::Duration) {
        self.beam_prev = self.beam_mode;
        self.beam_mode = mode;
        self.beam_blend = 0.0;
        self.beam_flash = Some((mode, Instant::now() + hold));
    }

    /// Per-frame: advance the blend and expire timed flashes.
    #[doc(hidden)]
    pub fn beam_tick(&mut self, dt: f32) {
        if self.beam_blend < 1.0 {
            self.beam_blend =
                (self.beam_blend + dt / style::BEAM_TRANSITION_SECONDS).min(1.0);
        }
        if let Some((_, until)) = self.beam_flash {
            if Instant::now() >= until {
                self.beam_flash = None;
                let base = self.beam_base;
                self.set_beam(base);
            }
        }
    }
}

/// The BUILT-IN component editors: the demo surface for the extension
/// system (a component can register its own — this covers the engine's
/// physics body). Sliders mutate the component live (the paused editor
/// re-renders after every edit); the action button defers engine work.
fn register_builtin_component_editors() {
    use crate::game::editor::component_editor::{
        register_component_editor, EditorAction, EditorFn,
    };
    let physics_body: EditorFn = |ui, component, origin| {
        let body = component
            .as_any_mut()
            .downcast_mut::<crate::game::physics_body::PhysicsBody>()?;
        ui.label(format!("body: {}", body.name()));
        ui.horizontal(|row| {
            row.label("gravity scale");
            row.add(
                egui::DragValue::new(&mut body.gravity_scale)
                    .speed(0.1)
                    .range(0.0..=10.0),
            );
        });
        ui.horizontal(|row| {
            row.label("mass");
            row.add(egui::DragValue::new(&mut body.mass).range(0.0..=100.0));
        });
        // the preprocess button: teleport the body's ENTITY to the
        // origin — deferred engine work through the action (a test of
        // the extension's in-editor preprocessing shape)
        if ui.button("reset position").clicked() {
            let path = origin.to_vec();
            return Some(EditorAction::Engine(Box::new(move |engine: &mut Engine| {
                if let Some(entity) = engine.entity_at_mut(&path) {
                    entity.transform_mut().position = Vec3::splat(0.0);
                }
            })));
        }
        None
    };
    register_component_editor::<crate::game::physics_body::PhysicsBody>(physics_body);
}

/// Depth-first search for an entity by name, appending child indices to
/// `path` as it descends. `Some(())` (with `path` completed) when found.
fn find_by_name(entity: &crate::game::Entity, want: &str, path: &mut Vec<usize>) -> bool {
    for (i, child) in entity.children().iter().enumerate() {
        path.push(i);
        if child.name() == want || find_by_name(child, want, path) {
            return true;
        }
        path.pop();
    }
    false
}

/// Internal message through the event-loop proxy (the M3 surface: pump
/// control messages ride winit's user events; nothing needs them yet).
enum ShellMessage {}

/// The camera preview's egui texture id (a User id — clear of egui's
/// managed font/atlas ids).
const PIP_TEXTURE: egui::TextureId = egui::TextureId::User(0xC0);

/// The pump-shared gates, passed through the UI pass.
#[doc(hidden)]
pub struct ShellShared {
    #[doc(hidden)]
    pub playing: Arc<AtomicBool>,
    #[doc(hidden)]
    pub dirty: Arc<AtomicBool>,
    /// File > Quit — the event loop watches it (egui's viewport Close
    /// command does not reach winit 0.30 through this embed).
    #[doc(hidden)]
    pub quit: Arc<AtomicBool>,
    /// The viewport panel's size — the pump renders the offscreen frame
    /// at it (the frame tracks the panel; no letterbox bars).
    #[doc(hidden)]
    pub frame_size: Arc<Mutex<[u32; 2]>>,
    /// The camera-preview camera (Some = render the preview into tag 1).
    #[doc(hidden)]
    pub pip_camera: Arc<Mutex<Option<crate::game::Camera>>>,
    /// The editor console (script errors from the pump, editor
    /// statuses) — the dock's Console tab reads it.
    #[doc(hidden)]
    pub console: Arc<Mutex<Vec<String>>>,
    /// The AI seam, in embryo: an embedding/agentic harness flips this
    /// while it works, and the top rainbow beam responds (hues rotate,
    /// hot spots blaze — the review's "processing or AI is connected
    /// and they're processing it"). Playing counts as processing too.
    /// The beam's ERROR trigger: the pump stores true when a script
    /// panics (hard red until the session clears it — the next play or
    /// save). The UI reads AND clears it each frame.
    #[doc(hidden)]
    pub error_beam: Arc<AtomicBool>,
    #[doc(hidden)]
    pub ai_busy: Arc<AtomicBool>,
}

impl ShellShared {
    /// Mark the scene changed — the paused pump re-renders once.
    fn touch(&self) {
        self.dirty.store(true, Ordering::SeqCst);
    }

    /// Push a line into the editor console (capped — the dock reads the
    /// tail).
    fn console_log(&self, message: impl Into<String>) {
        let mut console = self.console.lock().unwrap();
        console.push(message.into());
        if console.len() > 400 {
            let overflow = console.len().saturating_sub(400);
        console.drain(..overflow);
        }
    }
}

/// The running shell: the wgpu view, the egui halves, and the editor
/// state.
struct ShellHandler {
    engine: Arc<Mutex<Engine>>,
    sink: FrameSink,
    title: String,
    /// The pump's play gate (edit mode = paused; the UI toggles it).
    playing: Arc<AtomicBool>,
    /// The pump's re-render gate while paused (set by every edit).
    dirty: Arc<AtomicBool>,
    /// File > Quit (watched in about_to_wait).
    quit: Arc<AtomicBool>,
    /// The viewport panel's size (the pump renders at it).
    frame_size: Arc<Mutex<[u32; 2]>>,
    /// The camera-preview camera (Some = render the preview).
    pip_camera: Arc<Mutex<Option<crate::game::Camera>>>,
    /// The editor console (the dock's Console tab).
    console: Arc<Mutex<Vec<String>>>,
    /// The AI seam (the rainbow beam's second trigger).
    ai_busy: Arc<AtomicBool>,
    /// The beam's error trigger (script panics).
    error_beam: Arc<AtomicBool>,
    state: EditorState,
    window: Option<Arc<winit::window::Window>>,
    view: Option<ShellView>,
    egui_ctx: Option<egui::Context>,
    egui_state: Option<egui_winit::State>,
}

/// The editor-side state (everything that is not a wgpu resource).
/// `pub` for the headless growth-probe test only.
#[doc(hidden)]
pub struct EditorState {
    #[doc(hidden)]
    pub ui: EditorUi,
    mode: GizmoMode,
    drag: Option<DragState>,
    /// What the PRESS landed on (a gizmo handle claims the drag; empty
    /// space claims the pick on release — the two are decided at press).
    press_hit: Option<GizmoHit>,
    /// The active tab's scene file path — asset imports resolve against
    /// its directory. Doc-hidden pub: the headless probe tests set it.
    #[doc(hidden)]
    pub scene_path: String,
    dark: bool,
    /// The color buttons' paint (glass is the default; a View menu
    /// toggle swaps in the flat fills).
    button_style: style::ButtonStyle,
    /// The status line the bottom bar shows (save/load results, hints).
    status: String,
    fps_frames: u32,
    fps_since: Instant,
    fps: u32,
    /// The rainbow beam's clock — advances ONLY while something is
    /// processing (the hues rotate then; at rest the ribbon is still).
    beam_clock: f32,
    /// The last frame's egui time (the clock's delta source).
    beam_last: Option<f64>,
    /// The beam's TARGET mode; the paint blends from `beam_prev`.
    #[doc(hidden)]
    pub beam_mode: style::BeamMode,
    /// The mode the blend is leaving (the previous target).
    beam_prev: style::BeamMode,
    /// The blend ramp 0..1 toward `beam_mode` (seconds over
    /// BEAM_TRANSITION_SECONDS — quick, never instant).
    #[doc(hidden)]
    pub beam_blend: f32,
    /// A timed flash (mode + expiry): AutoSaving flickers, Saved hums,
    /// then the beam returns to `beam_base`.
    #[doc(hidden)]
    pub beam_flash: Option<(style::BeamMode, Instant)>,
    /// The session's rest mode (Rainbow normally; Error STICKS — the
    /// base stays red until the session clears it with a play/save).
    beam_base: style::BeamMode,
    /// The last autosave (the 30s dirty+saved rewrite cycle).
    #[doc(hidden)]
    pub autosave_last: Option<Instant>,
    /// The project document under edit (Some while Project Settings is
    /// open — the panel edits this copy; Save writes it beside the
    /// scene).
    project: Option<hyg::game::project::Project>,
    /// The panel's working text for the entry scene (an Option<String>
    /// can't bind a TextEdit; empty text = no entry).
    entry_text: String,
    /// The Project Settings window's open flag.
    show_project_settings: bool,
    /// Physical-pixel rect the engine frame letterboxes into (the
    /// picking/gizmo viewport).
    viewport: Option<Viewport>,
    /// The rename field's live buffer (Some while the text edit is
    /// focused — rebuilt-from-entity each frame would eat every
    /// keystroke).
    name_edit: Option<String>,
    /// Open scenes (tabs). One engine, one active scene at a time;
    /// switching loads the tab's document.
    tabs: Vec<SceneTab>,
    active_tab: usize,
    /// The play snapshot (the scene's document Var taken when Play was
    /// pressed — Stop/Restart restore it, so play never dirties the edit
    /// state).
    snapshot: Option<crate::Var>,
    /// The SCENE's camera — what the document saves and play mode uses.
    /// The editor's viewport uses its own orbit camera instead; this is
    /// the source of truth for save/play.
    scene_camera: crate::game::Camera,
    /// The editor's viewport camera (orbit).
    orbit: crate::game::editor::camera_ctl::Orbit,
    /// Some = free-fly camera (the View menu toggles); None = the orbit.
    fly: Option<crate::game::editor::camera_ctl::Fly>,
    /// The history panel's open flag.
    show_history: bool,
    /// The native file dialog in flight (None when idle).
    files: Option<NativeDialog>,
    /// The + component combo's pending choice (None = nothing picked).
    add_pick: Option<String>,
    /// The editor log (every status line, newest last, capped) — the
    /// dock's Log tab.
    log: Vec<String>,
    /// The hierarchy row being dragged (its path), for reparent-on-drop.
    dragged_row: Option<Vec<usize>>,
    /// The camera-preview window (tag-1 frames): shown while a camera
    /// entity is selected and its TRS is being edited, or locked.
    pip_open: bool,
    pip_locked: bool,
    seen_pip: u64,
    /// The camera component's fields changed this frame (the PiP's
    /// show trigger).
    camera_trs_editing: bool,
    /// Script file paths -> their mtime at last check (live reload).
    script_mtimes: std::collections::HashMap<std::path::PathBuf, std::time::SystemTime>,
    script_check_at: Option<Instant>,
    /// The bottom dock's active tab.
    dock_tab: DockTab,
    pip_last_edit: Option<Instant>,
}

/// The bottom dock's tabs. Console/debugger/profiler are staged homes —
/// their content ships with the milestones that own it (M3 script
/// console + debugger, M5 profiler); Log and Stats are live now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DockTab {
    Log,
    Stats,
    Console,
    /// The physics debug view (M4): the world's declared bodies + gravity.
    Physics,
    /// The audio debug view (M4): backend, master volume, play records.
    Audio,
    Debugger,
    Profiler,
}

/// One open scene (a tab): its label, its path once saved/loaded, and
/// whether it has unsaved edits.
struct SceneTab {
    label: String,
    path: Option<std::path::PathBuf>,
    dirty: bool,
    /// The tab's LIVE state (its full document) — snapshotted when the
    /// tab is deactivated, restored when it's activated. Tabs are
    /// independent in-memory scenes: unsaved edits survive switching.
    state: Option<crate::Var>,
}

/// A native file dialog in flight (rfd — xdg-desktop-portal on Linux,
/// the platform dialog everywhere else). The blocking call runs on a
/// worker thread; the UI polls `done` each frame.
struct NativeDialog {
    purpose: DialogPurpose,
    result: Arc<Mutex<Option<Option<std::path::PathBuf>>>>,
    done: Arc<AtomicBool>,
}

/// What the in-flight native dialog completes INTO — the poll matches
/// the completion to its flow (scene save/load, model import).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialogPurpose {
    SceneSave,
    SceneOpen,
    ModelImport,
}

impl EditorState {
    #[doc(hidden)]
    pub fn new(scene_camera: crate::game::Camera) -> EditorState {
        EditorState {
            scene_camera,
            orbit: crate::game::editor::camera_ctl::Orbit::default(),
            fly: None,
            ui: EditorUi::new(),
            mode: GizmoMode::Translate,
            drag: None,
            press_hit: None,
            scene_path: "scene.json.hyg".to_string(),
            dark: true,
            button_style: style::ButtonStyle::Glass,
            status: "click the viewport to pick · W/E/R move/rotate/scale · Ctrl+Z undo · Del deletes"
                .to_string(),
            fps_frames: 0,
            fps_since: Instant::now(),
            fps: 0,
            beam_clock: 0.0,
            beam_last: None,
            beam_mode: style::BeamMode::Rainbow,
            beam_prev: style::BeamMode::Rainbow,
            beam_blend: 1.0,
            beam_flash: None,
            beam_base: style::BeamMode::Rainbow,
            autosave_last: None,
            project: None,
            entry_text: String::new(),
            show_project_settings: false,
            viewport: None,
            name_edit: None,
            tabs: vec![SceneTab {
                label: "untitled".to_string(),
                path: None,
                dirty: false,
                state: None,
            }],
            active_tab: 0,
            snapshot: None,
            show_history: false,
            files: None,
            add_pick: None,
            log: Vec::new(),
            dragged_row: None,
            script_mtimes: std::collections::HashMap::new(),
            script_check_at: None,
            dock_tab: DockTab::Log,
            pip_open: true,
            pip_locked: false,
            seen_pip: 0,
            camera_trs_editing: false,
            pip_last_edit: None,
        }
    }

    /// Set the status line AND append it to the log (one seam).
    fn set_status(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.log.push(message.clone());
        if self.log.len() > 200 {
            self.log.drain(..self.log.len() - 200);
        }
        self.status = message;
    }

    fn tick_fps(&mut self) {
        self.fps_frames += 1;
        let elapsed = self.fps_since.elapsed();
        if elapsed >= Duration::from_millis(500) {
            self.fps = ((self.fps_frames as f64 / elapsed.as_secs_f64()).round()) as u32;
            self.fps_frames = 0;
            self.fps_since = Instant::now();
        }
    }

    /// Monotonic millis for the undo coalescing window (the pump pace is
    /// the clock's grain; exactness is irrelevant — the WINDOW is).
    fn now(&self) -> u64 {
        Instant::now().duration_since(self.fps_since).as_millis() as u64
            + u64::from(self.fps_frames) * 16
    }
}

/// An active gizmo drag (started on the press; deltas apply per frame;
/// the undo folds through the coalescing window).
enum DragState {
    TranslateAxis { axis: usize },
    TranslateCenter,
    Rotate { last_angle: f32 },
    ScaleAxis { axis: usize },
}

/// Per-window wgpu state: the engine-frame blit (the underlay pass, the
/// native view's shared machinery) plus the egui renderer (the overlay).
struct ShellView {
    blit: crate::game::native_view::BlitView,
    egui_renderer: egui_wgpu::Renderer,
    /// The last-uploaded sink generation (unchanged engine frames skip the
    /// upload; egui repaints regardless — the panels are live).
    seen: u64,
    /// The uploaded frame's dimensions (the letterbox fit + the pick
    /// viewport derive from them).
    frame_dims: [u32; 2],
}

impl ShellView {
    fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        self.blit.resize(size);
    }
}

impl winit::application::ApplicationHandler<ShellMessage> for ShellHandler {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let attributes = winit::window::Window::default_attributes()
            .with_title(self.title.clone())
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0))
            .with_maximized(true)
            .with_active(true);
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(e) => {
                eprintln!("studio: window creation failed: {e}");
                event_loop.exit();
                return;
            }
        };
        let view =
            match pollster::block_on(crate::game::native_view::build_blit_view(&window)) {
                Ok(blit) => {
                    let egui_renderer = egui_wgpu::Renderer::new(
                        &blit.device,
                        blit.config.format,
                        egui_wgpu::RendererOptions::default(),
                    );
                    ShellView {
                        blit,
                        egui_renderer,
                        seen: 0,
                        frame_dims: [1, 1],
                    }
                }
                Err(e) => {
                    eprintln!("studio: {e} — closing");
                    event_loop.exit();
                    return;
                }
            };
        let egui_ctx = egui::Context::default();
        style::apply(&egui_ctx, self.state.dark);
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        self.window = Some(Arc::clone(&window));
        self.view = Some(view);
        self.egui_ctx = Some(egui_ctx);
        self.egui_state = Some(egui_state);
        println!(
            "studio: \"{}\" — hierarchy left, inspector right, viewport center; \
             W/E/R gizmos, Ctrl+Z undo, Del deletes; close the window to quit",
            self.title
        );
        event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(16),
        ));
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        // egui sees every event first (the panels consume clicks in their
        // own territory); the shell keeps Esc/close as its exits
        if let (Some(egui_state), Some(window)) = (&mut self.egui_state, &self.window) {
            let response = egui_state.on_window_event(window, &event);
            if response.repaint {
                window.request_redraw();
            }
        }
        match event {
            winit::event::WindowEvent::CloseRequested => event_loop.exit(),
            winit::event::WindowEvent::KeyboardInput { event: key, .. } => {
                if key.state == winit::event::ElementState::Pressed
                    && matches!(
                        key.logical_key,
                        winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape)
                    )
                {
                    event_loop.exit();
                }
            }
            winit::event::WindowEvent::Resized(size) => {
                if let Some(view) = self.view.as_mut() {
                    view.resize(size);
                }
                // redraw NOW — resize edges track the mouse instead of
                // waiting for the next 16ms tick
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
                self.dirty.store(true, Ordering::SeqCst);
            }
            winit::event::WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.quit.load(Ordering::SeqCst) {
            event_loop.exit();
            return;
        }
        // pull the newest engine frame into the underlay texture; egui
        // repaints every display frame either way
        if let (Some(view), Some(window)) = (self.view.as_mut(), self.window.as_ref()) {
            if let Some(frame) = self.sink.latest_since(view.seen) {
                view.seen = frame.generation;
                view.frame_dims = [frame.width, frame.height];
                view.blit.upload(&frame);
            }
            window.request_redraw();
        }
        event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(16),
        ));
    }
}

impl ShellHandler {
    /// One display frame: the engine frame blits as the underlay pass,
    /// the egui panels composite as a load pass of the same view.
    fn redraw(&mut self) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let Some(egui_state) = self.egui_state.as_mut() else {
            return;
        };
        let Some(egui_ctx) = self.egui_ctx.as_ref() else {
            return;
        };
        self.state.tick_fps();
        let frame_dims = view.frame_dims;

        // egui pass: panels over the live engine (the lock is held while
        // the widgets read/write; the pump waits its turn — a value drag
        // costs a pump frame, nothing worse)
        let raw_input = egui_state.take_egui_input(window);
        egui_ctx.begin_pass(raw_input);
        {
            let mut engine = self.engine.lock().expect("studio engine");
            let shared = ShellShared {
                playing: Arc::clone(&self.playing),
                dirty: Arc::clone(&self.dirty),
                quit: Arc::clone(&self.quit),
                frame_size: Arc::clone(&self.frame_size),
                pip_camera: Arc::clone(&self.pip_camera),
                console: Arc::clone(&self.console),
                ai_busy: Arc::clone(&self.ai_busy),
                error_beam: Arc::clone(&self.error_beam),
            };
            draw_ui(egui_ctx, &mut self.state, &mut engine, frame_dims, &shared);
        }
        let mut output = egui_ctx.end_pass();
        egui_state.handle_platform_output(window, output.platform_output);

        // font/texture deltas land before the draw — `set` groups the
        // deltas per texture id; `mem::take` because TexturesDelta's Drop
        // forbids moving its fields out
        for (id, deltas) in std::mem::take(&mut output.textures_delta.set) {
            for delta in deltas {
                view.egui_renderer
                    .update_texture(&view.blit.device, &view.blit.queue, id, &delta);
            }
        }
        for id in std::mem::take(&mut output.textures_delta.free) {
            view.egui_renderer.free_texture(&id);
        }
        // the camera preview's frame (tag 1) uploads as a plain egui
        // texture — the PiP window paints it like any image
        if let Some(frame) = self.sink.latest_since_tag(self.state.seen_pip, 1) {
            self.state.seen_pip = frame.generation;
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.rgba,
            );
            let delta = egui::epaint::ImageDelta {
                image: egui::ImageData::Color(std::sync::Arc::new(image)),
                options: egui::TextureOptions::LINEAR,
                pos: None,
            };
            view.egui_renderer.update_texture(
                &view.blit.device,
                &view.blit.queue,
                PIP_TEXTURE,
                &delta,
            );
        }
        let primitives = egui_ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [view.blit.config.width, view.blit.config.height],
            pixels_per_point: output.pixels_per_point,
        };
        let surface_wh = (view.blit.config.width, view.blit.config.height);

        // the surface frame: pass 1 clears + blits the engine frame, pass
        // 2 loads + draws the egui meshes on top
        let surface_frame = match view.blit.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            _ => return, // skip: timeout/occluded/outdated/lost
        };
        let target_view = surface_frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = view
            .blit
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            // the frame maps exactly onto the viewport panel's rect
            // (last frame's layout — panels are stable between frames).
            // EXACT, not the letterbox rule: the frame is rendered at
            // the panel's size, so any rescale against the window's
            // aspect drifts the scene off the egui layer (the gizmos)
            match self.state.viewport {
                Some(vp_view) => view.blit.set_rect_exact(
                    surface_wh,
                    [vp_view.x, vp_view.y, vp_view.width, vp_view.height],
                ),
                None => view.blit.set_fit_rect(
                    (surface_wh.0, surface_wh.1),
                    (frame_dims[0].max(1), frame_dims[1].max(1)),
                    [0.0, 0.0],
                ),
            }
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hyg-studio-underlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            view.blit.blit_into(&mut pass);
        }
        let egui_buffers = view.egui_renderer.update_buffers(
            &view.blit.device,
            &view.blit.queue,
            &mut encoder,
            &primitives,
            &screen,
        );
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hyg-studio-egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            view.egui_renderer.render(&mut pass, &primitives, &screen);
        }
        view.blit
            .queue
            .submit(std::iter::once(encoder.finish()).chain(egui_buffers));
        view.blit.queue.present(surface_frame);
    }
}

/// Everything egui, top to bottom: the MENU BAR (File/Edit/View), the
/// TOOLBAR (transport, gizmo modes, scene tabs, fps), hierarchy left,
/// inspector right, a thin status bar bottom, and the transparent
/// viewport LAST (the panels carve the sides; what remains is the
/// scene's window). The engine lock is held across the whole pass.
fn draw_ui(
    ctx: &egui::Context,
    state: &mut EditorState,
    engine: &mut Engine,
    frame_dims: [u32; 2],
    shared: &ShellShared,
) {
    state.camera_trs_editing = false;

    // undo/redo + mode + delete keys — skipped while a text field owns
    // the keyboard
    if !ctx.egui_wants_keyboard_input() {
        let (undo_hit, redo_hit, mode_key, delete_hit) = ctx.input(|input| {
            (
                input.modifiers.ctrl && input.key_pressed(egui::Key::Z) && !input.modifiers.shift,
                (input.modifiers.ctrl && input.key_pressed(egui::Key::Y))
                    || (input.modifiers.ctrl
                        && input.modifiers.shift
                        && input.key_pressed(egui::Key::Z)),
                [
                    (egui::Key::W, GizmoMode::Translate),
                    (egui::Key::E, GizmoMode::Rotate),
                    (egui::Key::R, GizmoMode::Scale),
                ]
                .into_iter()
                .find(|(key, _)| input.key_pressed(*key))
                .map(|(_, mode)| mode),
                input.key_pressed(egui::Key::Delete),
            )
        });
        if undo_hit && state.ui.undo(engine) {
            state.set_status("undo");
            shared.touch();
        }
        if redo_hit && state.ui.redo(engine) {
            state.set_status("redo");
            shared.touch();
        }
        if let Some(mode) = mode_key {
            // fly mode consumes W/E as thrust keys — the gizmo-mode
            // shortcut yields while flying
            if state.fly.is_none() {
                state.mode = mode;
            }
        }
        if delete_hit && state.ui.delete_selected(engine, state.now()) {
            state.set_status("deleted (Ctrl+Z restores)");
            shared.touch();
        }
    }
    // Space toggles edit/play (guarded above — never while typing)
    let space_hit = !ctx.egui_wants_keyboard_input()
        && ctx.input(|input| input.key_pressed(egui::Key::Space) && !ctx.is_pointer_over_egui());
    if space_hit {
        if shared.playing.load(Ordering::SeqCst) {
            pause(state, shared);
        } else {
            play(state, engine, shared);
        }
    }

    // the editor's viewport camera: the orbit, not the scene's camera
    // (play mode swaps the scene camera in; save always writes
    // state.scene_camera). Aspect comes from the VIEWPORT panel — the
    // frame is rendered at the panel's size.
    let ppp = ctx.pixels_per_point();
    if !shared.playing.load(Ordering::SeqCst) {
        let (vw, vh) = match state.viewport {
            Some(view) => (view.width.max(1.0), view.height.max(1.0)),
            None => (
                frame_dims[0].max(1) as f32,
                frame_dims[1].max(1) as f32,
            ),
        };
        engine.set_camera(match state.fly {
            Some(fly) => fly.camera(vw / vh),
            None => state.orbit.camera(vw / vh),
        });
    }

    ctx.root_ui(|root| {
// the menu bar spans the top: menus left, transport FAR RIGHT (the
        // user's color language — play GREEN, stop RED, restart ORANGE)
        // the rainbow beam's clock advances ONLY while something is
        // processing (sim playing, or the AI seam busy) — at rest the
        // ribbon holds a calm, still rainbow
        let processing =
            shared.playing.load(Ordering::SeqCst) || shared.ai_busy.load(Ordering::SeqCst);
        let now = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt.max(1.0 / 240.0));
        if let Some(last) = state.beam_last {
            if processing {
                // clamp: a paused tab's resume jump shouldn't fast-forward
                state.beam_clock += (now - last).min(0.1) as f32;
            }
        }
        state.beam_last = Some(now);
        // the mode machine: blend ramp + timed flash expiry
        state.beam_tick(dt);
        // ERROR: the pump raises the flag on a script panic — the UI
        // reads AND clears it, and the beam goes HARD RED (the stick:
        // the base stays error until a play or save clears it)
        if shared
            .error_beam
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            state.beam_base = style::BeamMode::Error;
            state.set_beam(style::BeamMode::Error);
        }
        // AUTOSAVE: a saved scene with unsaved edits rewrites itself on
        // a cadence (never while playing) — the beam whispers
        {
            let tab_dirty = state
                .tabs
                .get(state.active_tab)
                .map(|t| t.dirty)
                .unwrap_or(false);
            let tab_path = state
                .tabs
                .get(state.active_tab)
                .and_then(|t| t.path.clone());
            if autosave_tick(state, engine, shared, tab_path.clone(), tab_dirty).is_ok()
                && tab_dirty
            {
                if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                    tab.dirty = false;
                }
            }
        }
        egui::containers::Panel::top("studio-menu")
            .exact_size(34.0)
            .show(root, |bar| {
                // the beam paints FIRST — background: the menus and
                // buttons render OVER it, and their drop shadows hold the
                // contrast against the light
                let bar_rect = bar.max_rect();
                let beam_rect = egui::Rect::from_min_size(
                    bar_rect.min,
                    egui::vec2(bar_rect.width(), style::BEAM_HEIGHT),
                );
                let beam_painter = bar.painter_at(beam_rect);
                style::paint_beam(
                    &beam_painter,
                    beam_rect,
                    state.beam_clock,
                    state.beam_prev,
                    state.beam_mode,
                    state.beam_blend,
                );
                bar.horizontal(|row| {
                    egui::MenuBar::new().ui(row, |menu| {
                        menu.menu_button("File", |file| {
                            if file.button("New Scene").clicked() {
                                new_scene(state, engine, shared);
                                file.close();
                            }
                            if file.button("Open...").clicked() {
                                open_file_dialog(state, DialogPurpose::SceneOpen);
                                file.close();
                            }
                            if file.button("Save").clicked() {
                                save_active_tab(state, engine);
                                file.close();
                            }
                            if file.button("Save As...").clicked() {
                                open_file_dialog(state, DialogPurpose::SceneSave);
                                file.close();
                            }
                            file.separator();
                            if file.button("Import Model...").clicked() {
                                open_file_dialog(state, DialogPurpose::ModelImport);
                                file.close();
                            }
                            if file.button("Export Game...").clicked() {
                                match locate_runtime()
                                    .and_then(|runtime| export_game(state, &runtime))
                                {
                                    Ok(out) => {
                                        println!("exported {}", out.display());
                                        state.flash_beam(
                                            style::BeamMode::Saved,
                                            std::time::Duration::from_millis(2000),
                                        );
                                    }
                                    Err(e) => state.set_status(format!("export: {e}")),
                                }
                                shared.touch();
                                file.close();
                            }
                            if file.button("Project Settings...").clicked() {
                                // load the document beside the scene (or
                                // a fresh default) into the panel's copy
                                let dir = std::path::PathBuf::from(&state.scene_path)
                                    .parent()
                                    .map(|p| p.to_path_buf())
                                    .unwrap_or_default();
                                state.project = Some(
                                    hyg::game::project::Project::load(&dir)
                                        .unwrap_or_default(),
                                );
                                state.entry_text = state
                                    .project
                                    .as_ref()
                                    .and_then(|p| p.entry.clone())
                                    .unwrap_or_default();
                                state.show_project_settings = true;
                                file.close();
                            }
                            file.separator();
                            if file.button("Quit").clicked() {
                                shared.quit.store(true, Ordering::SeqCst);
                            }
                        });
                        menu.menu_button("Edit", |edit| {
                            if edit
                                .add_enabled(
                                    state.ui.undo_stack().can_undo(),
                                    egui::Button::new("Undo"),
                                )
                                .clicked()
                                && state.ui.undo(engine)
                            {
                                state.set_status("undo");
                                shared.touch();
                                edit.close();
                            }
                            if edit
                                .add_enabled(
                                    state.ui.undo_stack().can_redo(),
                                    egui::Button::new("Redo"),
                                )
                                .clicked()
                                && state.ui.redo(engine)
                            {
                                state.set_status("redo");
                                shared.touch();
                                edit.close();
                            }
                            if edit
                                .add_enabled(
                                    state.ui.selection().is_some(),
                                    egui::Button::new("Delete"),
                                )
                                .clicked()
                                && state.ui.delete_selected(engine, state.now())
                            {
                                state.set_status("deleted (Ctrl+Z restores)");
                                mark_tab_dirty(state);
                                shared.touch();
                                edit.close();
                            }
                            edit.separator();
                            if edit.button("History").clicked() {
                                state.show_history = !state.show_history;
                                edit.close();
                            }
                        });
                        menu.menu_button("View", |view| {
                            if view
                                .button(if state.orbit.ortho {
                                    "Projection: Orthographic"
                                } else {
                                    "Projection: Perspective"
                                })
                                .clicked()
                            {
                                state.orbit.ortho = !state.orbit.ortho;
                                shared.touch();
                                view.close();
                            }
                            view.separator();
                            if view
                                .button(if state.dark { "Light theme" } else { "Dark theme" })
                                .clicked()
                            {
                                state.dark = !state.dark;
                                style::apply(view.ctx(), state.dark);
                                view.close();
                            }
                            if view
                                .button(match state.button_style {
                                    style::ButtonStyle::Glass => "Flat buttons",
                                    style::ButtonStyle::Flat => "Glass buttons",
                                })
                                .clicked()
                            {
                                state.button_style = match state.button_style {
                                    style::ButtonStyle::Glass => style::ButtonStyle::Flat,
                                    style::ButtonStyle::Flat => style::ButtonStyle::Glass,
                                };
                                view.close();
                            }
                            if view
                                .button(match state.fly {
                                    Some(_) => "orbit camera",
                                    None => "fly camera (WASD)",
                                })
                                .clicked()
                            {
                                // the toggle seeds the fly from the orbit's
                                // CURRENT pose — no jump at the switch
                                state.fly = match state.fly {
                                    None => Some(
                                        crate::game::editor::camera_ctl::Fly::from_camera(
                                            &state.orbit.camera(16.0 / 9.0),
                                        ),
                                    ),
                                    Some(_) => None,
                                };
                                state.set_status(match state.fly {
                                    Some(_) => {
                                        "fly camera - WASD move, QE up/down, RMB look, Shift boosts"
                                    }
                                    None => "orbit camera",
                                });
                                view.close();
                            }
                        });
                    });
                    // the transport, far right of the SAME row
                    row.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |right| {
                            // breathing room between the transport buttons
                            right.style_mut().spacing.item_spacing = egui::vec2(6.0, 4.0);
                            let playing_now = shared.playing.load(Ordering::SeqCst);
                            if style::color_button(
                                right,
                                "restart",
                                style::RESTART_ORANGE,
                                false,
                                true,
                                state.dark,
                                state.button_style,
                            )
                            .on_hover_text("restore the play snapshot and replay")
                            .clicked()
                            {
                                restart(state, engine, shared);
                            }
                            if style::color_button(
                                right,
                                "stop",
                                style::STOP_RED,
                                false,
                                true,
                                state.dark,
                                state.button_style,
                            )
                            .on_hover_text("back to the edit state")
                            .clicked()
                            {
                                stop(state, engine, shared);
                            }
                            if style::color_button(
                                right,
                                "step",
                                style::ACCENT_DIM,
                                false,
                                !playing_now,
                                state.dark,
                                state.button_style,
                            )
                            .on_hover_text("one frame while paused")
                            .clicked()
                            {
                                // one frame, exactly: update + draw + render
                                engine.update(1.0 / 60.0);
                                engine.draw();
                                engine.render();
                                state.set_status("stepped one frame");
                                shared.touch();
                            }
                            // one flip-flop: green "play" while paused, a
                            // glowing gold "pause" while the sim runs
                            if style::color_button(
                                right,
                                if playing_now { "pause" } else { "play" },
                                if playing_now {
                                    style::GOLD_DIM
                                } else {
                                    style::PLAY_GREEN
                                },
                                playing_now,
                                true,
                                state.dark,
                                state.button_style,
                            )
                            .on_hover_text(
                                "snapshot the edit state and run — flips to pause while the sim runs",
                            )
                            .clicked()
                            {
                                if playing_now {
                                    pause(state, shared);
                                } else {
                                    play(state, engine, shared);
                                }
                            }
                        },
                    );
                });
            });



        // the toolbar: gizmo modes, scene tabs, fps (transport moved to
        // the menu bar's right end)
        egui::containers::Panel::top("studio-toolbar")
            .exact_size(28.0)
            .show(root, |bar| {
                draw_toolbar(bar, state, engine, shared);
            });

        egui::containers::Panel::left("studio-hierarchy")
            .resizable(true)
            .size_range(160.0..=560.0)
            .default_size(240.0)
            .show(root, |panel| {
                panel.horizontal(|head| {
                    head.heading("Hierarchy");
                    // the create button: an obvious green + (glass like
                    // the rest of the chrome)
                    if style::color_button(
                        head,
                        "+",
                        style::CREATE_GREEN,
                        false,
                        true,
                        state.dark,
                        state.button_style,
                    )
                    .on_hover_text("new entity (child of the selection, else a root)")
                    .clicked()
                    {
                        // a child of the selection when one is live, else
                        // a new root
                        let name = format!("entity-{}", engine.entities().len() + 1);
                        let entity = Entity::new(name);
                        match state.ui.selection().map(|p| p.to_vec()) {
                            Some(path) if engine.entity_at(&path).is_some() => {
                                engine.entity_at_mut(&path).unwrap().add_child(entity);
                            }
                            _ => {
                                engine.add_entity(entity);
                            }
                        }
                        mark_tab_dirty(state);
                        shared.touch();
                    }
                });
                let nodes = EditorUi::hierarchy(engine);
                egui::ScrollArea::vertical()
                    .id_salt("studio-hierarchy-scroll")
                    .auto_shrink([false, false])
                    .show(panel, |scroll| {
                        // (path, rect) per row — the drop target lookup
                        let mut rows: Vec<(Vec<usize>, egui::Rect)> = Vec::new();
                        for node in &nodes {
                            hierarchy_rows(scroll, node, state, 0, &mut rows);
                        }
                        // a drag released over a row reparents; over the
                        // empty space below, the dragged entity goes to
                        // the roots
                        let reparent = state
                            .dragged_row
                            .clone()
                            .filter(|_| scroll.response().drag_stopped());
                        if let Some(from) = reparent {
                            state.dragged_row = None;
                            let pointer = scroll.response().interact_pointer_pos();
                            let target = pointer.and_then(|p| {
                                rows.iter()
                                    .find(|(path, rect)| {
                                        rect.contains(p) && *path != from
                                    })
                                    .map(|(path, _)| path.clone())
                            });
                            let landed = match target {
                                Some(to) => engine.reparent(&from, &to),
                                // drop on empty hierarchy space → a root
                                None => {
                                    let moved_to_root = (|| {
                                        let entity = engine.remove_entity(&from)?;
                                        engine.add_entity(entity);
                                        Some(())
                                    })();
                                    moved_to_root.is_some()
                                }
                            };
                            if landed {
                                state.set_status("reparented (drag again to move it back)");
                                mark_tab_dirty(state);
                                shared.touch();
                            }
                        }
                    });
            });

        draw_inspector_panel(root, state, engine, shared);

        // the bottom DOCK: log / stats / console / debugger / profiler —
        // the editor's output floor (Log and Stats live now; the others
        // are staged for the milestones that own their content)
        egui::containers::Panel::bottom("studio-dock")
            .resizable(true)
            .size_range(64.0..=420.0)
            .default_size(140.0)
            .show(root, |dock| {
                draw_dock(dock, state, engine, shared);
            });

        // the viewport: LAST, frameless so the blit shows through
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(root, |vp| {
                draw_viewport(vp, state, engine, ppp, shared, ctx);
            });
    });

    // the pump renders the frame AT the viewport panel's size (the blit
    // maps it back onto the panel — no letterbox bars anywhere)
    if let Some(view) = state.viewport {
        let desired = [view.width.round() as u32, view.height.round() as u32];
        let mut size = shared.frame_size.lock().unwrap();
        if desired[0] > 1 && desired[1] > 1 && (desired[0] != size[0] || desired[1] != size[1]) {
            *size = desired;
            shared.touch();
        }
    }

    // floating windows (history) live on the context, after panels
    if state.show_history {
        draw_history_window(ctx, state, engine, shared);
    }
    draw_project_settings_window(ctx, state, engine, shared);
    poll_file_dialog(state, engine, shared);

    // script live reload: poll mtimes ~1/sec; a changed file swaps the
    // component's source (Script::load — same id, host closures
    // re-bound, script state resets)
    let check_now = state
        .script_check_at
        .map(|t| t.elapsed() >= Duration::from_secs(1))
        .unwrap_or(true);
    if check_now {
        state.script_check_at = Some(Instant::now());
        for changed in reload_changed_scripts(engine, &mut state.script_mtimes) {
            shared.console_log(format!("reloaded {}", changed));
            state.set_status(format!("reloaded {changed}"));
            mark_tab_dirty(state);
            shared.touch();
        }
    }

    // the camera preview (PiP): shows while a camera entity is selected
    // AND its TRS is being edited (or the preview is locked) — the pump
    // renders its view into tag 1
    let camera_selected = state
        .ui
        .selection()
        .and_then(|path| engine.entity_at(path))
        .map(|entity| {
            entity
                .iter_components()
                .any(|c| c.as_any().downcast_ref::<crate::game::CameraComponent>().is_some())
        })
        .unwrap_or(false);
    let pip_visible = state.pip_open
        && camera_selected
        && (state.pip_locked
            || state.camera_trs_editing
            || state
                .pip_last_edit
                .map(|t| t.elapsed() < Duration::from_secs(1))
                .unwrap_or(false));
    let mut pip_camera = None;
    if pip_visible {
        if let Some(path) = state.ui.selection() {
            if let Some(entity) = engine.entity_at(path) {
                for component in entity.iter_components() {
                    if let Some(camera) = component
                        .as_any()
                        .downcast_ref::<crate::game::CameraComponent>()
                    {
                        pip_camera = Some(camera.camera);
                        break;
                    }
                }
            }
        }
    }
    *shared.pip_camera.lock().unwrap() = pip_camera;

    // the camera preview PiP window: the tag-1 texture, closable,
    // lockable (stays while locked)
    let pip_shown = pip_visible && state.seen_pip > 0;
    if pip_shown {
        egui::Window::new("camera preview")
            .open(&mut state.pip_open)
            .default_width(320.0)
            .show(ctx, |win| {
                win.horizontal(|head| {
                    let mut locked = state.pip_locked;
                    if head.checkbox(&mut locked, "lock").changed() {
                        state.pip_locked = locked;
                    }
                    if locked {
                        head.weak("(stays while locked)");
                    }
                });
                win.image(egui::ImageSource::Texture(egui::load::SizedTexture::new(
                    PIP_TEXTURE,
                    egui::vec2(320.0, 200.0),
                )));
            });
    }
}
// ---- transport: edit / play / stop / restart ----

/// The scene's main camera: the first entity carrying a main
/// [`CameraComponent`](crate::game::CameraComponent), walked tree-first.
#[must_use]
fn find_main_camera(engine: &Engine) -> Option<crate::game::Camera> {
    fn walk(entity: &Entity, found: &mut Option<crate::game::Camera>) {
        if found.is_some() {
            return;
        }
        for component in entity.iter_components() {
            if let Some(camera) = component
                .as_any()
                .downcast_ref::<crate::game::CameraComponent>()
            {
                if camera.main {
                    *found = Some(camera.camera);
                    return;
                }
            }
        }
        for child in entity.children() {
            walk(child, found);
        }
    }
    let mut found = None;
    for root in engine.entities() {
        walk(root, &mut found);
    }
    found
}

/// A camera that sees nothing — what plays when a scene has no main
/// camera (the render is blank, per the contract).
#[must_use]
fn blank_camera() -> crate::game::Camera {
    crate::game::Camera {
        position: Vec3::new(1.0e5, 1.0e5, 1.0e5),
        target: Vec3::new(0.0, 0.0, 0.0),
        projection: crate::game::camera::Projection::Perspective {
            fov_y_degrees: 60.0,
            near: 1.0,
            far: 2.0,
            aspect: 1.0,
        },
    }
}

/// Play: snapshot the edit state, resolve the MAIN camera (the scene's
/// dimensionality), then run. No main camera = a blank scene, per the
/// contract.
#[doc(hidden)]
pub fn play(state: &mut EditorState, engine: &mut Engine, shared: &ShellShared) {
    let label = state
        .tabs
        .get(state.active_tab)
        .map(|t| t.label.clone())
        .unwrap_or_else(|| "scene".to_string());
    // LOSSLESS: compiled models reference the session registry — the
    // file-shaped document would silently strip every procedural mesh
    // renderer (the restart bug: restore rebuilt entities with nothing
    // to draw)
    state.snapshot = Some(Scene::snapshot_document(
        &label,
        &state.scene_camera,
        engine.entities(),
    ));
    let main_camera = find_main_camera(engine);
    state.scene_camera = main_camera.unwrap_or_else(blank_camera);
    engine.set_camera(state.scene_camera);
    shared.playing.store(true, Ordering::SeqCst);
    // the beam: processing rainbow (and an Error stick CLEARS — the
    // session answered the alarm by running again)
    state.beam_base = style::BeamMode::Processing;
    state.set_beam(style::BeamMode::Processing);
    if main_camera.is_some() {
        state.set_status("playing — Space or pause to edit");
    } else {
        state.set_status("playing a BLANK scene — no main camera (add a camera entity)");
    }
}

/// Pause: stop the sim, keep the played state (resume from here).
fn pause(state: &mut EditorState, shared: &ShellShared) {
    shared.playing.store(false, Ordering::SeqCst);
    // the beam holds YELLOW while paused (the review's pause signal)
    state.beam_base = style::BeamMode::Paused;
    state.set_beam(style::BeamMode::Paused);
    state.set_status("paused — edits stick; play resumes");
    shared.touch();
}

/// Stop: back to the edit state (restore the play snapshot), sim off.
#[doc(hidden)]
pub fn stop(state: &mut EditorState, engine: &mut Engine, shared: &ShellShared) {
    shared.playing.store(false, Ordering::SeqCst);
    restore_snapshot(state, engine);
    // the beam rests: rainbow (an Error stick clears — the session
    // answered by returning to edit)
    state.beam_base = style::BeamMode::Rainbow;
    state.set_beam(style::BeamMode::Rainbow);
    state.set_status("stopped — back to the edit state");
    shared.touch();
}

/// Restart: restore the snapshot, then play again from the top.
#[doc(hidden)]
pub fn restart(state: &mut EditorState, engine: &mut Engine, shared: &ShellShared) {
    restore_snapshot(state, engine);
    play(state, engine, shared);
}

fn restore_snapshot(state: &mut EditorState, engine: &mut Engine) {
    if let Some(snapshot) = state.snapshot.clone() {
        // relative paths (scripts, models, sub-scenes) resolve against
        // the SCENE's directory — not wherever the process launched
        let base = scene_base(&state.scene_path);
        if let Ok(scene) = Scene::from_var_with_base(&snapshot, base) {
            engine.set_camera(scene.camera);
            engine.set_roots(scene.entities);
            state.name_edit = None;
        }
    }
}

/// The base directory a scene's file references resolve against: the
/// scene file's own directory (empty/untitled scenes use the working
/// directory, the loader's bare-`from_var` rule).
fn scene_base(path: impl AsRef<std::path::Path>) -> std::path::PathBuf {
    path.as_ref()
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

// ---- scene tabs ----

/// New Scene: a fresh untitled tab over an empty world.
fn new_scene(state: &mut EditorState, engine: &mut Engine, shared: &ShellShared) {
    shared.playing.store(false, Ordering::SeqCst);
    state.snapshot = None;
    let n = state.tabs.len() + 1;
    state.tabs.push(SceneTab {
        label: format!("untitled-{n}"),
        path: None,
        dirty: false,
        state: None,
    });
    activate_tab(state, engine, state.tabs.len() - 1);
    // every new scene is EMPTY except its main camera — the camera
    // decides the scene's dimensionality, defaulting to 3D (perspective;
    // the inspector's projection combo switches to 2D)
    let camera = crate::game::CameraComponent::default();
    state.scene_camera = camera.camera;
    state.orbit.ortho = matches!(
        camera.camera.projection,
        crate::game::camera::Projection::Orthographic { .. }
    );
    let mut camera_entity = Entity::new("main camera");
    camera_entity.add_component(Box::new(camera));
    engine.add_entity(camera_entity);
    shared.touch();
}

/// Activate a tab: load its document (an untitled tab is an empty world).
fn activate_tab(state: &mut EditorState, engine: &mut Engine, index: usize) {
    // switching tabs leaves play mode — the snapshot belongs to the
    // scene it was taken from (restart-after-switch used to replay the
    // PREVIOUS scene's snapshot)
    state.snapshot = None;
    // copy the incoming tab's bits up front (the outgoing snapshot
    // below re-borrows `tabs` mutably)
    let (incoming_path, incoming_state) = match state.tabs.get(index) {
        Some(tab) => (tab.path.clone(), tab.state.clone()),
        None => return,
    };
    // the OUTGOING tab keeps its live tree (unsaved edits survive the
    // switch — tabs are independent in-memory scenes)
    if state.active_tab < state.tabs.len() {
        let label = state.tabs[state.active_tab].label.clone();
        let scene_camera = state.scene_camera;
        state.tabs[state.active_tab].state =
            Some(Scene::document(&label, &scene_camera, engine.entities()));
    }
    state.active_tab = index;
    state.name_edit = None;
    // the incoming tab: its live state if it has one, else its document
    // (the document's relative paths resolve against the TAB's scene
    // directory — the same rule snapshot restore follows)
    if let Some(doc) = &incoming_state {
        let base = incoming_path
            .as_deref()
            .map(scene_base)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        if let Ok(scene) = Scene::from_var_with_base(doc, base) {
            state.scene_camera = scene.camera;
            engine.set_camera(scene.camera);
            engine.set_roots(scene.entities);
            state.orbit.ortho = matches!(
                state.scene_camera.projection,
                crate::game::camera::Projection::Orthographic { .. }
            );
            return;
        }
    }
    match &incoming_path {
        Some(path) => {
            state.scene_path = path.to_string_lossy().to_string();
            if load_scene(state, engine).is_err() {
                engine.set_roots(Vec::new());
            }
        }
        None => {
            engine.set_roots(Vec::new());
        }
    }
}

/// Save the active tab: to its path when it has one, else Save As.
fn save_active_tab(state: &mut EditorState, engine: &mut Engine) {
    let path = state.tabs.get(state.active_tab).and_then(|t| t.path.clone());
    match path {
        Some(path) => {
            match save_scene(&path, &state.scene_camera.clone(), engine) {
                Ok(()) => {
                    state.scene_path = path.to_string_lossy().to_string();
                    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                        tab.dirty = false;
                    }
                    state.set_status(format!("saved {}", path.to_string_lossy()));
                }
                Err(e) => state.set_status(format!("save failed: {e}")),
            }
        }
        None => {
            state.set_status("save as — name the scene");
            open_file_dialog(state, DialogPurpose::SceneSave);
        }
    }
}

/// A scene landed on disk (from the dialog): open it as a NEW tab and
/// activate it.
fn open_scene_path(state: &mut EditorState, engine: &mut Engine, path: &Path, shared: &ShellShared) {
    let label = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("scene")
        .trim_end_matches(".json")
        .to_string();
    state.tabs.push(SceneTab {
        label,
        path: Some(path.to_path_buf()),
        dirty: false,
        state: None,
    });
    activate_tab(state, engine, state.tabs.len() - 1);
    shared.touch();
}

/// The bottom dock: a tab strip and its content. Log is the status
/// history; Stats is live engine telemetry; the staged tabs name their
/// milestone (the dock is the extension point for more).
fn draw_dock(
    dock: &mut egui::Ui,
    state: &mut EditorState,
    engine: &mut Engine,
    shared: &ShellShared,
) {
    dock.horizontal(|tabs| {
        for (tab, label) in [
            (DockTab::Log, "Log"),
            (DockTab::Stats, "Stats"),
            (DockTab::Console, "Console"),
            (DockTab::Physics, "Physics"),
            (DockTab::Audio, "Audio"),
            (DockTab::Debugger, "Debugger"),
            (DockTab::Profiler, "Profiler"),
        ] {
            if style::tab_button(
                tabs,
                label,
                state.dock_tab == tab,
                state.dark,
                state.button_style,
            )
            .clicked()
            {
                state.dock_tab = tab;
            }
        }
    });
    // the divider TOUCHES the tab row: scoped zero item spacing (the
    // default 8pt gap read as a floating line)
    dock.scope(|sep| {
        sep.style_mut().spacing.item_spacing = egui::Vec2::ZERO;
        sep.add(egui::Separator::default().spacing(2.0));
    });
    match state.dock_tab {
        DockTab::Log => {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(dock, |scroll| {
                    if state.log.is_empty() {
                        scroll.weak("(nothing logged yet)");
                    }
                    for line in &state.log {
                        scroll.monospace(egui::RichText::new(line).small());
                    }
                });
        }
        DockTab::Stats => {
            let playing = shared.playing.load(Ordering::SeqCst);
            let entities: usize = engine.entities().iter().map(count_subtree).sum();
            let components: usize = engine
                .entities()
                .iter()
                .map(|e| e.iter_components().count() + count_child_components(e))
                .sum();
            dock.monospace(format!(
                "mode: {}   fps: {}\nroot entities: {}   total entities: {}   components: {}",
                if playing { "playing" } else { "edit" },
                state.fps,
                engine.entities().len(),
                entities,
                components,
            ));
        }
        DockTab::Console => {
            // the unified feed: script panics (from the pump) first, then
            // the editor's statuses — newest last
            let console_lines = shared.console.lock().unwrap().clone();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(dock, |scroll| {
                    if console_lines.is_empty() && state.log.is_empty() {
                        scroll.weak("(nothing yet — script panics and editor statuses land here)");
                    }
                    for line in &console_lines {
                        scroll.monospace(
                            egui::RichText::new(line).color(style::dark::DANGER).small(),
                        );
                    }
                    for line in &state.log {
                        scroll.monospace(egui::RichText::new(line).small());
                    }
                });
        }
        DockTab::Physics => {
            // the read-only rows: what the world DECLARES. The simulation
            // stays the backend's — the view reads its records (a no-op
            // backend's declarations are real data too: a scene's bodies
            // show even while transforms stay under script control)
            match engine.physics() {
                Some(world) => {
                    let snap = crate::game::editor::debug::PhysicsSnapshot::from_world(world);
                    dock.monospace(format!(
                        "backend: {}   gravity: {}",
                        snap.backend,
                        match snap.gravity {
                            Some(g) => format!("{:.2}, {:.2}, {:.2}", g.x, g.y, g.z),
                            None => "(backend reports none)".to_string(),
                        }
                    ));
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(dock, |scroll| {
                            if snap.bodies.is_empty() {
                                scroll.weak("(no bodies registered)");
                            }
                            for body in &snap.bodies {
                                scroll.monospace(
                                    egui::RichText::new(format!(
                                        "{}   {}   {}   at ({:.2}, {:.2}, {:.2})",
                                        body.name,
                                        shape_label(&body.shape),
                                        body.body_type.as_str(),
                                        body.position.x,
                                        body.position.y,
                                        body.position.z,
                                    ))
                                    .small(),
                                );
                            }
                        });
                }
                None => {
                    dock.weak("no physics world attached (set_physics attaches one)");
                }
            }
        }
        #[cfg(feature = "audio")]
        DockTab::Audio => {
            match engine.audio() {
                Some(world) => {
                    let snap = crate::game::editor::debug::AudioSnapshot::from_world(world);
                    dock.monospace(format!("backend: {}", snap.backend));
                    // master volume: the slider writes straight through
                    let mut volume = snap.master_volume.unwrap_or(1.0);
                    let slider = dock.add(
                        egui::Slider::new(&mut volume, 0.0..=1.0).text("master volume"),
                    );
                    if slider.changed() {
                        if let Some(world) = engine.audio_mut() {
                            world.set_master_volume(volume);
                        }
                        shared.touch();
                    }
                    if dock.button("play test tone (440 Hz)").clicked() {
                        let tone = crate::game::audio::AudioSource::Tone {
                            frequency: 440.0,
                            duration: 0.2,
                            shape: crate::game::audio::WaveShape::Sine,
                        };
                        match engine
                            .audio_mut()
                            .expect("checked Some above")
                            .play(&tone, &crate::game::audio::PlaySpec::new(0.8, false))
                        {
                            Ok(id) => state.set_status(format!("tone playing (id {id})")),
                            Err(e) => state.set_status(format!("tone failed: {e}")),
                        }
                        shared.touch();
                    }
                    dock.separator();
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(dock, |scroll| match snap.plays {
                            Some(plays) if plays.is_empty() => {
                                scroll.weak("(no plays recorded)");
                            }
                            Some(plays) => {
                                for play in &plays {
                                    scroll.monospace(
                                        egui::RichText::new(format!(
                                            "#{}   {}   vol {:.2}{}",
                                            play.id,
                                            source_label(&play.source),
                                            play.volume,
                                            if play.looping { "  looping" } else { "" },
                                        ))
                                        .small(),
                                    );
                                }
                            }
                            None => {
                                scroll.weak("(this backend keeps no play records)");
                            }
                        });
                }
                None => {
                    dock.weak("no audio world attached (set_audio attaches one)");
                }
            }
        }
        #[cfg(not(feature = "audio"))]
        DockTab::Audio => {
            dock.weak("the audio view rides the `audio` feature");
        }
        DockTab::Debugger => {
            dock.weak("graph runs attach here — the debugger panel ships with the dynamic seam UI");
        }
        DockTab::Profiler => {
            dock.weak("frame profiling ships with M5 (polish)");
        }
    }
}

fn count_subtree(entity: &Entity) -> usize {
    1 + entity.children().iter().map(count_subtree).sum::<usize>()
}

/// The autosave tick: a SAVED scene with unsaved edits rewrites
/// itself every 30s (never while playing — the played state is the
/// sim's, not the author's). The beam flickers AutoSaving during the
/// write and hums green after; failures land in the status line.
/// `#[doc(hidden)] pub` for the headless growth-probe tests.
#[doc(hidden)]
pub fn autosave_tick(
    state: &mut EditorState,
    engine: &mut Engine,
    shared: &ShellShared,
    path: Option<std::path::PathBuf>,
    dirty: bool,
) -> Result<(), String> {
    if shared.playing.load(Ordering::SeqCst) {
        return Ok(());
    }
    let due = state
        .autosave_last
        .map(|t| t.elapsed() >= std::time::Duration::from_secs(30))
        .unwrap_or(true);
    if !(due && dirty && path.is_some()) {
        return Ok(());
    }
    state.autosave_last = Some(Instant::now());
    state.flash_beam(
        style::BeamMode::AutoSaving,
        std::time::Duration::from_millis(600),
    );
    let path = path.expect("checked Some");
    let result = save_scene(&path, &state.scene_camera, engine);
    match &result {
        Ok(()) => {
            state.set_status(format!("autosaved {}", path.display()));
            state.flash_beam(
                style::BeamMode::Saved,
                std::time::Duration::from_millis(1200),
            );
        }
        Err(e) => state.set_status(format!("autosave failed: {e}")),
    }
    shared.touch();
    result
}

/// A collision shape as the physics view's row text.
fn shape_label(shape: &crate::game::physics::Shape) -> String {
    match shape {
        crate::game::physics::Shape::Box(h) => {
            format!("box {:.2}x{:.2}x{:.2} (half)", h.x, h.y, h.z)
        }
        crate::game::physics::Shape::Sphere(r) => format!("sphere r={r:.2}"),
    }
}

/// An audio source as the audio view's row text.
#[cfg(feature = "audio")]
fn source_label(source: &crate::game::audio::AudioSource) -> String {
    match source {
        crate::game::audio::AudioSource::File(path) => format!("file {path}"),
        crate::game::audio::AudioSource::Tone {
            frequency,
            duration,
            shape,
        } => format!(
            "tone {:.0} Hz {:.2}s {}",
            frequency,
            duration,
            match shape {
                crate::game::audio::WaveShape::Sine => "sine",
                crate::game::audio::WaveShape::Square => "square",
                crate::game::audio::WaveShape::Noise => "noise",
            },
        ),
    }
}

fn count_child_components(entity: &Entity) -> usize {
    entity
        .children()
        .iter()
        .map(|c| c.iter_components().count() + count_child_components(c))
        .sum()
}

/// Walk the tree; for every file-backed scripted component whose source
/// file's mtime moved since the last check, reload it. Returns the
/// reloaded paths (errors log loudly and keep the old script — a failed
/// reload is never a dead editor).
fn reload_changed_scripts(
    engine: &mut Engine,
    mtimes: &mut std::collections::HashMap<std::path::PathBuf, std::time::SystemTime>,
) -> Vec<String> {
    fn walk(
        entity: &mut Entity,
        mtimes: &mut std::collections::HashMap<std::path::PathBuf, std::time::SystemTime>,
        reloaded: &mut Vec<String>,
    ) {
        if let Some(scripted) = entity.component_mut::<crate::game::ScriptedComponent>()
        {
            if let Some(path) = scripted.source_path() {
                let path = std::path::PathBuf::from(path);
                let current = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                match (current, mtimes.get(&path).copied()) {
                    (Some(now), Some(seen)) if now > seen => {
                        match scripted.reload_from_file() {
                            Ok(()) => reloaded.push(path.to_string_lossy().to_string()),
                            Err(e) => reloaded.push(format!("{path:?} FAILED: {e}")),
                        }
                        mtimes.insert(path, now);
                    }
                    (Some(now), None) => {
                        mtimes.insert(path, now);
                    }
                    _ => {}
                }
            }
        }
        for child in entity.children_mut() {
            walk(child, mtimes, reloaded);
        }
    }
    let mut reloaded = Vec::new();
    for root in engine.entities_mut() {
        walk(root, mtimes, &mut reloaded);
    }
    reloaded
}

/// The toolbar: gizmo modes, scene tabs, fps.
fn draw_toolbar(
    bar: &mut egui::Ui,
    state: &mut EditorState,
    engine: &mut Engine,
    shared: &ShellShared,
) {
    let _ = engine;
    bar.horizontal(|row| {
        // the gizmo modes, color-coded to their gizmo axes of heart:
        // move red, rotate green, scale blue (the active mode wears the
        // pressed/engaged glow)
        for (mode, color) in [
            (GizmoMode::Translate, egui::Color32::from_rgb(232, 85, 95)),
            (GizmoMode::Rotate, egui::Color32::from_rgb(105, 212, 107)),
            (GizmoMode::Scale, egui::Color32::from_rgb(91, 141, 240)),
        ] {
            if style::color_button(
                row,
                mode.name(),
                color,
                state.mode == mode,
                true,
                state.dark,
                state.button_style,
            )
            .clicked()
            {
                state.mode = mode;
            }
        }

        row.separator();
        // scene tabs: click to switch, x to close, + to open
        let mut close_tab: Option<usize> = None;
        let mut switch_tab: Option<usize> = None;
        for (i, tab) in state.tabs.iter().enumerate() {
            let label = if tab.dirty {
                format!("{} *", tab.label)
            } else {
                tab.label.clone()
            };
            if style::tab_button(row, &label, i == state.active_tab, state.dark, state.button_style)
                .clicked()
            {
                switch_tab = Some(i);
            }
            if row.small_button("x").clicked() {
                close_tab = Some(i);
            }
        }
        if row.small_button("+").clicked() {
            open_file_dialog(state, DialogPurpose::SceneOpen);
        }
        if let Some(i) = switch_tab {
            if i != state.active_tab {
                let dirty_warning = state
                    .tabs
                    .get(state.active_tab)
                    .map(|t| t.dirty)
                    .unwrap_or(false);
                activate_tab(state, engine, i);
                if dirty_warning {
                    state.set_status("switched tabs — the previous tab had unsaved edits");
                }
            }
        }
        if let Some(i) = close_tab {
            if state.tabs.len() > 1 {
                state.tabs.remove(i);
                let next = i.min(state.tabs.len() - 1);
                activate_tab(state, engine, next);
            } else {
                state.set_status("the last tab stays — File > New Scene for a fresh one");
            }
        }

        row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |right| {
            right.label(format!("{} fps", state.fps));
            let mode = if shared.playing.load(Ordering::SeqCst) {
                "playing"
            } else {
                "edit"
            };
            right.label(mode);
            right.separator();
            right.weak(&state.status);
        });
    });
}

/// The viewport pass: picking, gizmo interaction, gizmo painting. All
/// math in physical pixels over the SURFACE letterbox rect; egui's
/// pointer arrives in points (absolute, window-origin) and converts by
/// `ppp`.
fn draw_viewport(
    vp: &mut egui::Ui,
    state: &mut EditorState,
    engine: &mut Engine,
    ppp: f32,
    shared: &ShellShared,
    ctx: &egui::Context,
) {
    let response = vp.allocate_rect(vp.max_rect(), egui::Sense::click_and_drag());
    // the viewport IS the central panel: the pump renders the frame at
    // this exact size and the blit maps it back onto this exact rect —
    // no letterbox bars, gizmos glued to the pixels
    let max = vp.max_rect();
    let vp_rect = Viewport::new(
        max.min.x * ppp,
        max.min.y * ppp,
        max.width() * ppp,
        max.height() * ppp,
    );
    state.viewport = Some(vp_rect);

    let camera = *engine.camera();

    // the gizmo layout for the current selection (recomputed per frame —
    // the entity moves under the drag). The gizmo's world size rides the
    // orbit distance: zoom rescales it with the geometry (rendered
    // THROUGH the MVP, per the review)
    // the gizmo's size is FIXED IN WORLD UNITS: rendered through the
    // MVP it scales with the viewport's zoom like the geometry (the
    // review's ask; a distance-scaled length would cancel the zoom and
    // pin the gizmo at constant screen size)
    let gizmo_world_len = gizmo::GIZMO_WORLD_LEN;
    let layout: Option<GizmoLayout> = state.ui.selection().and_then(|path| {
        let world = engine.world_transform_at(path)?;
        gizmo::layout(
            state.mode,
            &camera,
            &vp_rect,
            world.position,
            world.rotation,
            gizmo_world_len,
        )
    });

    let pointer_phys = response
        .interact_pointer_pos()
        .map(|p| [p.x * ppp, p.y * ppp]);
    let drag_px = response.drag_delta();
    let drag_phys = [drag_px.x * ppp, drag_px.y * ppp];

    // the editor camera: right-drag LOOKS while flying, orbits
    // otherwise; middle-drag pans and the wheel zooms are orbit tools
    // (while the pointer is over the viewport)
    if response.dragged_by(egui::PointerButton::Secondary) {
        if let Some(fly) = state.fly.as_mut() {
            fly.look(drag_px.x, drag_px.y);
        } else {
            state.orbit.orbit_by(drag_px.x, drag_px.y);
        }
        shared.touch();
    }
    if response.dragged_by(egui::PointerButton::Middle) {
        state.orbit.pan_by(&camera, drag_px.x, drag_px.y);
        shared.touch();
    }
    if response.hovered() {
        let wheel = ctx_input_wheel(ctx);
        if wheel != 0.0 {
            state.orbit.zoom_by(wheel);
            shared.touch();
        }
    }
    // fly thrust: WASD + QE (Shift boosts) while the pointer is over
    // the viewport; dt from egui keeps the speed frame-rate honest
    if let Some(fly) = state.fly.as_mut() {
        if response.hovered() {
            let dt = ctx.input(|i| i.stable_dt.max(1.0 / 240.0));
            let boost = if ctx.input(|i| i.modifiers.shift) { 3.0 } else { 1.0 };
            let mut thrusts = (0.0f32, 0.0f32, 0.0f32);
            ctx.input(|i| {
                if i.key_down(egui::Key::W) {
                    thrusts.0 += 1.0;
                }
                if i.key_down(egui::Key::S) {
                    thrusts.0 -= 1.0;
                }
                if i.key_down(egui::Key::D) {
                    thrusts.1 += 1.0;
                }
                if i.key_down(egui::Key::A) {
                    thrusts.1 -= 1.0;
                }
                if i.key_down(egui::Key::E) {
                    thrusts.2 += 1.0;
                }
                if i.key_down(egui::Key::Q) {
                    thrusts.2 -= 1.0;
                }
            });
            let (fwd, strafe, up) = thrusts;
            if fwd != 0.0 || strafe != 0.0 || up != 0.0 {
                fly.thrust(fwd, strafe, up, dt * boost);
                shared.touch();
            }
        }
    }

    // the press decides its claim: a gizmo handle claims the drag, empty
    // space claims the pick on release
    if response.is_pointer_button_down_on() && state.press_hit.is_none() && state.drag.is_none() {
        state.press_hit = pointer_phys
            .zip(layout.as_ref())
            .map(|(pointer, layout)| gizmo::hit_test(layout, pointer))
            .or(Some(GizmoHit::None));
    }
    if !response.is_pointer_button_down_on() {
        state.press_hit = None;
    }

    // gizmo drag start
    if response.drag_started() {
        if let (Some(hit), Some(layout), Some(pointer)) =
            (state.press_hit, layout.as_ref(), pointer_phys)
        {
            state.drag = match hit {
                GizmoHit::Axis(axis) => Some(match state.mode {
                    GizmoMode::Translate => DragState::TranslateAxis { axis },
                    GizmoMode::Scale => DragState::ScaleAxis { axis },
                    GizmoMode::Rotate => DragState::Rotate {
                        last_angle: pointer_angle(pointer, layout.center),
                    },
                }),
                GizmoHit::Center => Some(DragState::TranslateCenter),
                GizmoHit::Ring => Some(DragState::Rotate {
                    last_angle: pointer_angle(pointer, layout.center),
                }),
                GizmoHit::None => None,
            };
        }
    }

    // per-frame delta application (the undo coalescing folds the drag)
    if response.dragged() {
        if let Some(drag) = state.drag.as_ref() {
            match drag {
                DragState::TranslateAxis { axis } => {
                    if let (Some(layout), Some(entity)) = (
                        layout.as_ref(),
                        state
                            .ui
                            .selected_entity(engine)
                            .map(|e| e.transform().position),
                    ) {
                        if let Some(handle) = layout.axes.iter().find(|h| h.axis == *axis) {
                            let delta = gizmo::translate_delta(handle, drag_phys);
                            state
                                .ui
                                .set_selected_position(engine, entity + delta, state.now());
                            shared.touch();
                        }
                    }
                }
                DragState::TranslateCenter => {
                    if let Some(entity) = state
                        .ui
                        .selected_entity(engine)
                        .map(|e| e.transform().position)
                    {
                        let delta =
                            gizmo::translate_center_delta(&camera, &vp_rect, entity, drag_phys);
                        state
                            .ui
                            .set_selected_position(engine, entity + delta, state.now());
                        shared.touch();
                    }
                }
                DragState::ScaleAxis { axis } => {
                    if let (Some(layout), Some(scale)) = (
                        layout.as_ref(),
                        state
                            .ui
                            .selected_entity(engine)
                            .map(|e| e.transform().scale),
                    ) {
                        if let Some(handle) = layout.axes.iter().find(|h| h.axis == *axis) {
                            let delta = gizmo::scale_delta(handle, drag_phys);
                            let mut scale = scale;
                            let component = match axis {
                                0 => &mut scale.x,
                                1 => &mut scale.y,
                                _ => &mut scale.z,
                            };
                            *component = (*component + delta).max(0.01);
                            state.ui.set_selected_scale(engine, scale, state.now());
                            shared.touch();
                        }
                    }
                }
                DragState::Rotate { last_angle } => {
                    if let (Some(layout), Some(pointer), Some(rotation)) = (
                        layout.as_ref(),
                        pointer_phys,
                        state
                            .ui
                            .selected_entity(engine)
                            .map(|e| e.transform().rotation),
                    ) {
                        let angle = pointer_angle(pointer, layout.center);
                        let mut d = angle - *last_angle;
                        if d > std::f32::consts::PI {
                            d -= std::f32::consts::TAU;
                        }
                        if d < -std::f32::consts::PI {
                            d += std::f32::consts::TAU;
                        }
                        let (forward, _, _) = gizmo::view_basis(&camera);
                        let delta = gizmo::rotate_delta(d, forward);
                        state
                            .ui
                            .set_selected_rotation(engine, delta.compose(rotation), state.now());
                        shared.touch();
                        if let Some(DragState::Rotate { last_angle }) = state.drag.as_mut() {
                            *last_angle = angle;
                        }
                    }
                }
            }
        }
    }
    if response.drag_stopped() {
        state.drag = None;
    }

    // the pick: a release without a gizmo claim (no drag) selects
    if response.clicked() && state.press_hit == Some(GizmoHit::None) {
        if let Some(pointer) = pointer_phys {
            match pick::click_pick(engine, &camera, &vp_rect, pointer) {
                Some(path) => {
                    // clicking the already-selected entity deselects it
                    if state.ui.selection() == Some(path.as_slice()) {
                        state.ui.deselect();
                        state.set_status("deselected");
                    } else {
                        let name = engine
                            .entity_at(&path)
                            .map(|e| e.name().to_string())
                            .unwrap_or_default();
                        state.ui.select(path);
                        state.set_status(format!("selected {name}"));
                    }
                }
                None => {
                    state.ui.deselect();
                    state.set_status("deselected");
                }
            }
        }
    }

    // the viewport options, top-right: the projection switch (live) and
    // the render modes (staged — named for their milestone until the
    // renderer grows them)
    let corner = egui::Rect::from_min_size(
        egui::pos2(vp.max_rect().right() - 158.0, vp.max_rect().top() + 4.0),
        egui::vec2(154.0, 20.0),
    );
    let current = if state.orbit.ortho { "ortho" } else { "persp" };
    let button = vp.put(
        corner,
        egui::Button::new(format!("view: {current}")),
    );
    egui::Popup::menu(&button).show(|popup| {
        if popup
            .selectable_label(!state.orbit.ortho, "perspective")
            .clicked()
        {
            state.orbit.ortho = false;
            shared.touch();
        }
        if popup.selectable_label(state.orbit.ortho, "orthographic (2D)").clicked() {
            state.orbit.ortho = true;
            shared.touch();
        }
        popup.separator();
        popup.add_enabled(false, egui::Button::new("wireframe (render modes: M5)"));
        popup.add_enabled(false, egui::Button::new("normals (render modes: M5)"));
        popup.add_enabled(false, egui::Button::new("ambient occlusion (M5)"));
    });

    // paint the gizmo (points = physical ÷ ppp; the painter clips to the
    // viewport ui)
    if let Some(layout) = &layout {
        paint_gizmo(vp.painter().clone(), layout, state.mode, ppp, state.drag.is_some());
    }
}

/// The viewport's wheel delta (points), 0 when none.
fn ctx_input_wheel(ctx: &egui::Context) -> f32 {
    ctx.input(|input| input.smooth_scroll_delta.y)
}

/// The pointer's angle around a center (atan2 in y-down screen space —
/// the rotation ring's contract).
#[must_use]
fn pointer_angle(pointer: [f32; 2], center: [f32; 2]) -> f32 {
    (pointer[1] - center[1]).atan2(pointer[0] - center[0])
}

/// The gizmo's paint pass — colored axis lines with arrowheads, the
/// center square, the rotation ring. Physical px in, egui points out.
pub fn paint_gizmo(
    painter: egui::Painter,
    layout: &GizmoLayout,
    mode: GizmoMode,
    ppp: f32,
    dragging: bool,
) {
    let to_points = |p: [f32; 2]| egui::pos2(p[0] / ppp, p[1] / ppp);
    let center = to_points(layout.center);
    let colors = [
        egui::Color32::from_rgb(232, 85, 95),   // x
        egui::Color32::from_rgb(105, 212, 107), // y
        egui::Color32::from_rgb(91, 141, 240),  // z
    ];
    if mode != GizmoMode::Rotate {
        for handle in &layout.axes {
            let mut color = colors[handle.axis];
            if handle.foreshortened {
                color = color.gamma_multiply(0.4);
            }
            let end = to_points(handle.screen_end);
            painter.line_segment(
                [center, end],
                egui::Stroke::new(if dragging { 3.0 } else { 2.0 }, color),
            );
            // arrowhead: a small triangle pointing along the axis
            let perp = [-handle.screen_dir[1], handle.screen_dir[0]];
            let head = 8.0;
            let tip = end;
            let base = [
                end[0] - handle.screen_dir[0] * head / ppp,
                end[1] - handle.screen_dir[1] * head / ppp,
            ];
            let b1 = egui::pos2(
                base[0] + perp[0] * head * 0.6 / ppp,
                base[1] + perp[1] * head * 0.6 / ppp,
            );
            let b2 = egui::pos2(
                base[0] - perp[0] * head * 0.6 / ppp,
                base[1] - perp[1] * head * 0.6 / ppp,
            );
            painter.add(egui::Shape::convex_polygon(
                vec![tip, b1, b2],
                color,
                egui::Stroke::NONE,
            ));
            // the axis's letter just past its tip — with overlapping
            // perspectives, color alone is not enough to know which
            // arrow is which
            let letter = ["X", "Y", "Z"][handle.axis];
            let label_pos = egui::pos2(
                end.x + handle.screen_dir[0] * 12.0,
                end.y + handle.screen_dir[1] * 12.0,
            );
            painter.text(
                label_pos,
                egui::Align2::CENTER_CENTER,
                letter,
                egui::FontId::proportional(12.0),
                color,
            );
        }
    }
    match mode {
        GizmoMode::Translate => {
            // the center square: view-plane move
            let half = 5.0;
            let min = egui::pos2(center.x - half, center.y - half);
            let max = egui::pos2(center.x + half, center.y + half);
            painter.rect_filled(egui::Rect::from_min_max(min, max), 2.0, style::dark::GOLD);
        }
        GizmoMode::Rotate => {
            let radius = layout.ring_radius / ppp;
            painter.circle_stroke(center, radius, egui::Stroke::new(2.0, colors[1]));
            painter.circle_filled(center, 3.0, egui::Color32::WHITE);
        }
        GizmoMode::Scale => {
            let half = 4.0;
            let min = egui::pos2(center.x - half, center.y - half);
            let max = egui::pos2(center.x + half, center.y + half);
            painter.rect_stroke(
                egui::Rect::from_min_max(min, max),
                2.0,
                egui::Stroke::new(1.5, egui::Color32::WHITE),
                egui::StrokeKind::Inside,
            );
        }
    }
}

/// The hierarchy rows: indent, (off) for disabled, component tags, click
/// to select, the selection in gold.
#[allow(clippy::too_many_arguments)]
fn hierarchy_rows(
    panel: &mut egui::Ui,
    node: &crate::game::editor::ui::HierarchyNode,
    state: &mut EditorState,
    depth: usize,
    rows: &mut Vec<(Vec<usize>, egui::Rect)>,
) {
    let selected = state.ui.selection() == Some(node.path.as_slice());
    let mut label = String::new();
    label.push_str(&"    ".repeat(depth));
    if !node.enabled {
        label.push_str("(off) ");
    }
    label.push_str(&node.name);
    if !node.component_types.is_empty() {
        label.push_str(&format!("   · {}", node.component_types.join(", ")));
    }
    // click-and-drag rows: click selects (again = deselects), drag
    // carries the row for reparenting
    let response = panel.add(
        egui::Button::new(label)
            .fill(if selected {
                style::GOLD_DIM
            } else {
                egui::Color32::TRANSPARENT
            })
            .min_size(egui::vec2(panel.available_width(), 14.0)),
    );
    rows.push((node.path.clone(), response.rect));
    if response.clicked() {
        if selected {
            state.ui.deselect();
            state.set_status("deselected");
        } else {
            state.ui.select(node.path.clone());
            state.name_edit = None;
            state.set_status(format!("selected {}", node.name));
        }
    }
    if response.drag_started() {
        state.dragged_row = Some(node.path.clone());
    }
    for child in &node.children {
        hierarchy_rows(panel, child, state, depth + 1, rows);
    }
}

/// Rows of DragValues filling the available width: vec3 → one row of 3,
/// quaternion → 2×2. Widths derive from the row's available space, so
/// narrow panels shrink the fields instead of spilling.
fn axis_row(
    scroll: &mut egui::Ui,
    labels: &[&str],
    values: &mut [f32],
    speed: f32,
    changed: &mut bool,
) {
    let per_row = if values.len() == 4 { 2 } else { values.len() };
    for (row, chunk) in values.chunks_mut(per_row).enumerate() {
        let row_labels = &labels[row * per_row..row * per_row + chunk.len()];
        scroll.horizontal(|line| {
            // FIXED field width — any width derived from
            // available_width inside a resizable panel chases the
            // panel's own chrome every frame (the growth loop; the
            // label column + spacing under-counted and ballooned the
            // panel to its cap). Constants can't feed back; 72px fits
            // "-123.45".
            let width = 72.0;
            for (value, label) in chunk.iter_mut().zip(row_labels) {
                let mut v = f64::from(*value);
                let drag = egui::DragValue::new(&mut v).speed(speed);
                // the axis letter rides OUTSIDE the field, color-coded
                // (x red, y green, z blue, w amber) — matching the gizmo
                let axis_color = match *label {
                    "x" => egui::Color32::from_rgb(232, 85, 95),
                    "y" => egui::Color32::from_rgb(105, 212, 107),
                    "z" => egui::Color32::from_rgb(91, 141, 240),
                    _ => style::dark::GOLD,
                };
                line.label(
                    egui::RichText::new(format!(" {label}"))
                        .color(axis_color)
                        .strong(),
                );
                let response = line.add_sized([width, 18.0], drag);
                if response.changed() {
                    *value = v as f32;
                    *changed = true;
                }
            }
        });
    }
}

/// The + component combo: picks a registered type, spawns it on the
/// selected entity with default params. `true` when one landed.
fn add_component_ui(
    scroll: &mut egui::Ui,
    state: &mut EditorState,
    engine: &mut Engine,
    path: &[usize],
) -> bool {
    let types: Vec<String> = crate::game::scene::registered_component_types()
        .into_iter()
        .filter(|t| t != "script")
        .collect();
    let mut picked = state.add_pick.clone();
    egui::ComboBox::from_id_salt("studio-add-component")
        .selected_text(picked.clone().unwrap_or_else(|| "+ component".to_string()))
        .width(160.0)
        .show_ui(scroll, |combo| {
            for ty in &types {
                combo.selectable_value(&mut picked, Some(ty.clone()), ty);
            }
        });
    state.add_pick = picked.clone();
    let Some(ty) = picked else {
        return false;
    };
    let Some(name) = engine.entity_at(path).map(|e| e.name().to_string()) else {
        return false;
    };
    let ctx = crate::game::scene::ComponentContext::new(name);
    // the editor's spawn defaults — the FACTORIES stay strict (documents
    // must be complete); the UI fills the minimum viable params so a
    // fresh component lands ready to edit
    let default_params = match ty.as_str() {
        "physics_body" => Var::from(o! {
            "shape": Var::from("box"),
            "half_extents": Var::from(a![Var::from(0.5), Var::from(0.5), Var::from(0.5)]),
        }),
        "mesh_renderer" => Var::from(o! { "shape": Var::from("rect") }),
        _ => crate::Var::from(crate::Object::new()),
    };
    match crate::game::scene::create_component(&ty, &default_params, &ctx) {
        Ok(component) => {
            if let Some(entity) = engine.entity_at_mut(path) {
                entity.add_component(component);
                state.set_status(format!("added {ty}"));
                state.add_pick = None;
                true
            } else {
                false
            }
        }
        Err(e) => {
            state.set_status(format!("{ty} needs more than defaults: {e}"));
            false
        }
    }
}

/// The inspector: name, enabled, transform (one row per vec3, 2×2 for
/// the quaternion — filling the panel), components with their params,
/// delete. Every write is an undo step. Scrolled.
/// The inspector's right panel (separate so the growth-probe test can
/// drive the REAL panel headlessly). Returns the panel's width.
#[doc(hidden)]
pub fn draw_inspector_panel(
    root: &mut egui::Ui,
    state: &mut EditorState,
    engine: &mut Engine,
    shared: &ShellShared,
) -> f32 {
    egui::containers::Panel::right("studio-inspector")
        .resizable(true)
        .size_range(200.0..=620.0)
        .default_size(300.0)
        .show(root, |panel| {
            draw_inspector(panel, state, engine, shared);
        })
        .response
        .rect
        .width()
}

fn draw_inspector(
    panel: &mut egui::Ui,
    state: &mut EditorState,
    engine: &mut Engine,
    shared: &ShellShared,
) {
    panel.heading("Inspector");
    let Some(path) = state.ui.selection().map(|p| p.to_vec()) else {
        state.name_edit = None;
        panel.label("nothing selected — click the viewport or the hierarchy");
        return;
    };
    // the display values copy OUT of the tree — the borrow ends here,
    // so the component-editor walk below can take the engine mutably
    // (custom editors mutate their components live)
    let (name, enabled, transform) = {
        let Some(entity) = engine.entity_at(&path) else {
            state.ui.deselect();
            state.name_edit = None;
            panel.label("(stale selection — the tree moved)");
            return;
        };
        (
            entity.name().to_string(),
            entity.enabled(),
            *entity.transform(),
        )
    };

    egui::ScrollArea::vertical()
        .id_salt("studio-inspector-scroll")
        .auto_shrink([false, false])
        .show(panel, |scroll| {
            // rename: the buffer persists in the editor state while the
            // field is focused (rebuilt-from-entity each frame would eat
            // every keystroke); committed on blur or Enter
            let mut buf = state.name_edit.clone().unwrap_or_else(|| name.clone());
            // finite width — an INFINITY desired width balloons a
            // resizable panel to its max
            // FIXED desired width — anything derived from
            // available_width inside a resizable panel chases the
            // panel's own chrome every frame (the growth loop)
            let edit = scroll.add(
                egui::TextEdit::singleline(&mut buf).desired_width(180.0),
            );
            if edit.has_focus() {
                state.name_edit = Some(buf.clone());
            }
            if edit.lost_focus() {
                state.name_edit = None;
                let trimmed = buf.trim().to_string();
                if !trimmed.is_empty() && trimmed != name {
                    state.ui.rename_selected(engine, &trimmed, state.now());
                    mark_tab_dirty(state);
                    shared.touch();
                }
            }
            let mut enabled_buf = enabled;
            if scroll.checkbox(&mut enabled_buf, "enabled").changed() {
                state.ui.set_selected_enabled(engine, enabled_buf, state.now());
                mark_tab_dirty(state);
                shared.touch();
            }

            scroll.separator();
            scroll.strong("transform");
            let mut changed = false;

            scroll.label("position");
            let mut pos = [transform.position.x, transform.position.y, transform.position.z];
            axis_row(scroll, &["x", "y", "z"], &mut pos, 0.05, &mut changed);
            if changed {
                let pos = Vec3::new(pos[0], pos[1], pos[2]);
                state.ui.set_selected_position(engine, pos, state.now());
                mark_tab_dirty(state);
                shared.touch();
            }

            let mut changed = false;
            scroll.label("scale");
            let mut scale = [transform.scale.x, transform.scale.y, transform.scale.z];
            axis_row(scroll, &["x", "y", "z"], &mut scale, 0.05, &mut changed);
            if changed {
                let scale = Vec3::new(scale[0], scale[1], scale[2]);
                state.ui.set_selected_scale(engine, scale, state.now());
                mark_tab_dirty(state);
                shared.touch();
            }

            let mut changed = false;
            scroll.label("rotation (quaternion)");
            let mut rotation = [
                transform.rotation.x,
                transform.rotation.y,
                transform.rotation.z,
                transform.rotation.w,
            ];
            axis_row(scroll, &["x", "y", "z", "w"], &mut rotation, 0.01, &mut changed);
            if changed {
                let rotation = Quat::new(rotation[0], rotation[1], rotation[2], rotation[3]);
                state.ui.set_selected_rotation(engine, rotation, state.now());
                mark_tab_dirty(state);
                shared.touch();
            }

            scroll.separator();
            scroll.strong("components");
            // components WITH a registered editor extension get egui
            // access (sliders, buttons for in-editor preprocessing —
            // the review: "a test or convert button"); the rest keep
            // the generic parameter summary. Deferred actions run
            // after the mutable walk (the borrow never overlaps the
            // engine's).
            let mut deferred: Vec<crate::game::editor::component_editor::EditorAction> =
                Vec::new();
            if let Some(entity_mut) = engine.entity_at_mut(&path) {
                let count = entity_mut.iter_components_mut().count();
                let mut any = count > 0;
                for index in 0..count {
                    let component = entity_mut
                        .iter_components_mut()
                        .nth(index)
                        .expect("index within the component count");
                    let type_name = component.scene_type().unwrap_or("(runtime only)");
                    let registered =
                        crate::game::editor::component_editor::editor_for_component(component);
                    match registered {
                        Some(editor) => {
                            scroll.strong(type_name.to_string());
                            if let Some(action) = editor(scroll, component, &path) {
                                deferred.push(action);
                            }
                        }
                        None => {
                            any = true;
                            let ctx = crate::game::scene::ComponentContext::new(&name);
                            let params = component.scene_params(&ctx);
                            let detail = match &params {
                                Some(var) => match var.as_object() {
                                    Some(object) if !object.is_empty() => {
                                        let keys: Vec<String> = object
                                            .entries()
                                            .iter()
                                            .map(|(k, _)| k.clone())
                                            .collect();
                                        format!("  · {type_name}  [{}]", keys.join(", "))
                                    }
                                    _ => format!("  · {type_name}  (defaults)"),
                                },
                                None => format!("  · {type_name}"),
                            };
                            scroll.label(detail);
                        }
                    }
                }
                if !any {
                    scroll.label("  (none)");
                }
            }
            for action in deferred {
                action.run(engine);
                shared.touch();
            }

            // add a component: the scene registry's types, spawned with
            // DEFAULT params (script is file-backed — it comes with the
            // script editor milestone, not a default-params spawn)
            if add_component_ui(scroll, state, engine, &path) {
                mark_tab_dirty(state);
                shared.touch();
            }

            // the CAMERA component's controls (projection = the scene's
            // 2D/3D switch; main = what play renders through). Direct
            // mutation for v1 — camera-field undo commands come with the
            // M3 snapshot work.
            let mut camera_changed = false;
            if let Some(entity) = engine.entity_at_mut(&path) {
                if let Some(camera) = entity.component_mut::<crate::game::CameraComponent>() {
                    scroll.separator();
                    scroll.strong("camera");
                    let mut main_buf = camera.main;
                    if scroll.checkbox(&mut main_buf, "main scene camera").changed() {
                        camera.main = main_buf;
                        camera_changed = true;
                    }
                    let is_ortho =
                        matches!(camera.camera.projection, Projection::Orthographic { .. });
                    let label = if is_ortho {
                        "orthographic (2D)"
                    } else {
                        "perspective (3D)"
                    };
                    egui::ComboBox::from_id_salt("studio-camera-projection")
                        .selected_text(label)
                        .width(scroll.available_width().min(180.0))
                        .show_ui(scroll, |combo| {
                            if combo.selectable_label(!is_ortho, "perspective (3D)").clicked() {
                                camera.camera.projection = Projection::Perspective {
                                    fov_y_degrees: 60.0,
                                    near: 0.1,
                                    far: 500.0,
                                    aspect: 16.0 / 9.0,
                                };
                                camera_changed = true;
                            }
                            if combo.selectable_label(is_ortho, "orthographic (2D)").clicked() {
                                camera.camera.projection = Projection::Orthographic {
                                    half_extents: Vec3::new(8.89, 5.0, 500.0),
                                };
                                camera_changed = true;
                            }
                        });
                    match &mut camera.camera.projection {
                        Projection::Orthographic { half_extents } => {
                            scroll.label("half extents");
                            let mut hx_new = f64::from(half_extents.x);
                            let mut hy_new = f64::from(half_extents.y);
                            let mut hz_new = f64::from(half_extents.z);
                            let mut hz_changed = false;
                            for (axis, v) in [
                                ("x", &mut hx_new),
                                ("y", &mut hy_new),
                                ("z", &mut hz_new),
                            ] {
                                if scroll
                                    .add(
                                        egui::DragValue::new(v)
                                            .speed(0.1)
                                            .prefix(format!("{axis} ")),
                                    )
                                    .changed()
                                {
                                    hz_changed = true;
                                }
                            }
                            if hz_changed {
                                half_extents.x = hx_new as f32;
                                half_extents.y = hy_new as f32;
                                half_extents.z = hz_new as f32;
                            }
                            camera_changed = camera_changed || hz_changed;
                        }
                        Projection::Perspective {
                            fov_y_degrees,
                            near,
                            far,
                            ..
                        } => {
                            let mut fov = f64::from(*fov_y_degrees);
                            if scroll
                                .add(
                                    egui::DragValue::new(&mut fov)
                                        .speed(0.5)
                                        .prefix("fov "),
                                )
                                .changed()
                            {
                                *fov_y_degrees = fov as f32;
                                camera_changed = true;
                            }
                            let mut near_v = f64::from(*near);
                            if scroll
                                .add(
                                    egui::DragValue::new(&mut near_v)
                                        .speed(0.01)
                                        .prefix("near "),
                                )
                                .changed()
                            {
                                *near = near_v as f32;
                                camera_changed = true;
                            }
                            let mut far_v = f64::from(*far);
                            if scroll
                                .add(
                                    egui::DragValue::new(&mut far_v)
                                        .speed(1.0)
                                        .prefix("far "),
                                )
                                .changed()
                            {
                                *far = far_v as f32;
                                camera_changed = true;
                            }
                        }
                    }
                    if camera_changed && camera.main {
                        // the main camera IS the scene's camera — keep the
                        // editor's copy in lockstep (save/play read it)
                        state.scene_camera = camera.camera;
                        state.orbit.ortho = is_ortho;
                    }
                }
            }
            if camera_changed {
                state.camera_trs_editing = true;
                state.pip_last_edit = Some(Instant::now());
                mark_tab_dirty(state);
                shared.touch();
            }

            scroll.separator();
            if scroll
                .add(egui::Button::new("delete").fill(style::dark::DANGER))
                .clicked()
                && state.ui.delete_selected(engine, state.now())
            {
                state.set_status("deleted (Ctrl+Z restores)");
                mark_tab_dirty(state);
                shared.touch();
            }
        });
}

/// Mark the active tab edited (the tab's `*`).
fn mark_tab_dirty(state: &mut EditorState) {
    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
        tab.dirty = true;
    }
}

/// The scene save (path from the dialog): the M0 canonical document of
/// the live tree, the name from the path stem, the format from the
/// filename.
fn save_scene(path: &Path, scene_camera: &crate::game::Camera, engine: &Engine) -> Result<(), String> {
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("untitled")
        .trim_end_matches(".json")
        .to_string();
    // the SCENE camera, never the editor's orbit view
    let doc = Scene::document(&name, scene_camera, engine.entities());
    carrier::write(path, &doc).map_err(|e| e.to_string())
}

/// The scene load (path from the active tab / dialog): the carrier
/// reads, the scene materializes, the engine replaces its roots and
/// camera. The undo stack clears (stored paths moved with the old tree)
/// and the selection resets.
fn load_scene(state: &mut EditorState, engine: &mut Engine) -> Result<(), String> {
    let scene = Scene::load(Path::new(&state.scene_path)).map_err(|e| e.to_string())?;
    engine.set_camera(scene.camera);
    engine.set_roots(scene.entities);
    state.ui = EditorUi::new();
    state.drag = None;
    state.name_edit = None;
    // the scene's dimensionality follows its main camera component (the
    // top-level camera is the fallback)
    let main = find_main_camera(engine);
    state.scene_camera = main.unwrap_or(scene.camera);
    state.orbit.ortho = matches!(
        state.scene_camera.projection,
        crate::game::camera::Projection::Orthographic { .. }
    );
    Ok(())
}

/// A save name gets its `.hyg` terminal suffix if missing — the carrier
/// refuses anything else, so the dialog repairs it instead of failing.
#[must_use]
pub fn ensure_hyg_suffix(name: &str) -> String {
    if name.ends_with(".hyg") {
        name.to_string()
    } else if name.ends_with(".json") {
        format!("{name}.hyg")
    } else {
        format!("{name}.json.hyg")
    }
}

/// Open the NATIVE file dialog (save or load) on a worker thread; the
/// UI polls for the result each frame.
fn open_file_dialog(state: &mut EditorState, purpose: DialogPurpose) {
    let default_name = match purpose {
        DialogPurpose::SceneSave => state
            .tabs
            .get(state.active_tab)
            .map(|t| {
                t.path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| format!("{}.json.hyg", t.label))
            })
            .unwrap_or_else(|| "scene.json.hyg".to_string()),
        DialogPurpose::SceneOpen | DialogPurpose::ModelImport => String::new(),
    };
    let result: Arc<Mutex<Option<Option<std::path::PathBuf>>>> = Arc::new(Mutex::new(None));
    let done = Arc::new(AtomicBool::new(false));
    let worker = {
        let result = Arc::clone(&result);
        let done = Arc::clone(&done);
        std::thread::Builder::new()
            .name("hyg-studio-file-dialog".to_string())
            .spawn(move || {
                let picked = match purpose {
                    DialogPurpose::SceneSave => rfd::FileDialog::new()
                        .add_filter("hyg scene", &["hyg"])
                        .set_file_name(&default_name)
                        .save_file(),
                    DialogPurpose::SceneOpen => rfd::FileDialog::new()
                        .add_filter("hyg scene", &["hyg"])
                        .pick_file(),
                    DialogPurpose::ModelImport => rfd::FileDialog::new()
                        .add_filter("glTF model", &["glb", "gltf"])
                        .pick_file(),
                };
                *result.lock().unwrap() = Some(picked);
                done.store(true, Ordering::SeqCst);
            })
            .ok()
    };
    if worker.is_none() {
        state.set_status("file dialog thread failed to spawn");
        return;
    }
    state.files = Some(NativeDialog {
        purpose,
        result,
        done,
    });
}

/// The M4 model import: copy the picked `.glb`/`.gltf` into the active
/// scene's `assets/` directory (the scene references ITS OWN tree, not
/// the original location), spawn a root entity carrying an asset-backed
/// renderer (path relative to the scene file), select it. Under the
/// `gltf` feature the copy is parse-checked and the renderer materializes
/// immediately — a broken file refuses the import loudly instead of
/// landing a blank entity. Headless-testable: the dialog is the only UI.
///
/// Returns the copied file name for the status line.
///
/// # Errors
/// The copy or (gltf) the parse fails — the message names the path.
#[doc(hidden)]
pub fn import_model(
    state: &mut EditorState,
    engine: &mut Engine,
    src: &std::path::Path,
) -> Result<String, String> {
    let scene_dir = std::path::Path::new(&state.scene_path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let Some(name) = src.file_name() else {
        return Err(format!("{} has no file name", src.display()));
    };

    // parse-check BEFORE anything lands (gltf builds): the import is
    // loud — a bad file copies nowhere and spawns nothing
    #[cfg(feature = "gltf")]
    {
        let probe = crate::game::Model::from_gltf_file(src)
            .map_err(|e| format!("{}: {e}", src.display()))?;
        let (_, half) = probe.bounds();
        if half.x + half.y + half.z <= 0.0 {
            return Err(format!("{} contains no vertices", src.display()));
        }
    }

    let assets = scene_dir.join("assets");
    std::fs::create_dir_all(&assets).map_err(|e| format!("assets dir: {e}"))?;
    let dest = assets.join(name);
    if src != dest {
        std::fs::copy(src, &dest).map_err(|e| format!("copy: {e}"))?;
    }
    let rel_path = format!("assets/{}", name.to_string_lossy());

    let stem = dest
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model")
        .to_string();
    let mut entity = Entity::new(&stem);
    // spawn where the editor is looking (the orbit target — the new
    // model is in frame immediately)
    entity.transform_mut().position = state.orbit.target;
    entity.add_component(Box::new(
        crate::game::renderable::MeshRenderer::asset(rel_path.clone(), [1.0, 1.0, 1.0, 1.0]),
    ));
    engine.add_entity(entity);

    // materialize now: no scene reload needed for the first look
    #[cfg(feature = "gltf")]
    {
        let path = vec![engine.entities().len() - 1];
        if let Some(at) = engine.entity_at_mut(&path) {
            if let Some(renderer) = at.component_mut::<crate::game::renderable::MeshRenderer>() {
                // the entity stays even on a resolution failure (the file
                // copied fine) — the status names it; the renderer keeps
                // its path
                renderer.resolve(&scene_dir)?;
            }
        }
    }

    state.ui.select(vec![engine.entities().len() - 1]);
    Ok(name.to_string_lossy().to_string())
}

/// The M5 export: the WHOLE packaging flow inside the Studio — bundle
/// the saved scene with its references (the M4 walk), fuse it onto the
/// runtime binary, and land ONE game file beside the scene. The
/// runtime binary is located via `HYG_RUNTIME`, then
/// `target/release/hyg-runtime`, then `target/debug/hyg-runtime`
/// (beside the crate manifest and the working directory) — the status
/// line names the build command when none is found.
///
/// Returns the fused game's path for the status line.
///
/// # Errors
/// The scene isn't saved yet, the runtime binary isn't found, or the
/// bundle/fuse fails — every message says what to do next.
#[doc(hidden)]
pub fn export_game(
    state: &mut EditorState,
    runtime: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let scene_path = std::path::PathBuf::from(&state.scene_path);
    if !scene_path.exists() {
        return Err("save the scene first — Export packages the SAVED file".to_string());
    }
    let scene_dir = scene_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let stem = scene_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("scene")
        .trim_end_matches(".json")
        .to_string();

    // bundle BESIDE the scene (visible artifact, same layout the CLI
    // produces), then fuse onto the runtime — ONE file out
    let staging = scene_dir.join(format!("{stem}-bundle"));
    let report = hyg::bundle::build(&scene_path, &staging)?;
    let out = scene_dir.join(format!("{stem}.bin"));
    let size = hyg::pack::fuse(runtime, &staging, &out)?;
    state.set_status(format!(
        "exported {} ({} files, {} KB) — the single file plays the game",
        out.display(),
        report.files.len(),
        size / 1024
    ));
    Ok(out)
}

/// Locate the runtime binary for exports: `HYG_RUNTIME` overrides,
/// then the cargo target dirs (beside the crate manifest and the
/// working directory). The error names the build command — the
/// approachable failure tells the newcomer what to do, not just what
/// broke.
///
/// # Errors
/// None of the candidates exists.
#[doc(hidden)]
pub fn locate_runtime() -> Result<std::path::PathBuf, String> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("HYG_RUNTIME") {
        candidates.push(std::path::PathBuf::from(p));
    }
    candidates.push(std::path::PathBuf::from("target/release/hyg-runtime"));
    candidates.push(std::path::PathBuf::from("target/debug/hyg-runtime"));
    if let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") {
        candidates.push(std::path::PathBuf::from(dir).join("target/release/hyg-runtime"));
    }
    candidates
        .into_iter()
        .find(|p| p.exists())
        .ok_or_else(|| {
            "no runtime binary found — build it once: cargo build --release --bin hyg-runtime --features wgpu,winit,script,gltf,physics,audio,pack (or set HYG_RUNTIME)".to_string()
        })
}

/// The Project Settings window: the game's identity and defaults
/// (name, version, entry scene, window shape) edited in one panel and
/// written to `project.json.hyg` beside the scene — the document
/// `hyg build`/`hyg fuse` and the runtime all read.
fn draw_project_settings_window(
    ctx: &egui::Context,
    state: &mut EditorState,
    engine: &mut Engine,
    shared: &ShellShared,
) {
    if !state.show_project_settings {
        return;
    }
    let Some(open) = state.project.as_mut() else {
        return;
    };
    let mut save = false;
    egui::Window::new("Project Settings")
        .open(&mut state.show_project_settings)
        .resizable(false)
        .default_size([320.0, 200.0])
        .show(ctx, |panel| {
            egui::Grid::new("project-grid")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(panel, |grid| {
                    grid.label("name");
                    grid.add(
                        egui::TextEdit::singleline(&mut open.name)
                            .desired_width(180.0),
                    );
                    grid.end_row();
                    grid.label("version");
                    grid.add(
                        egui::TextEdit::singleline(&mut open.version)
                            .desired_width(180.0),
                    );
                    grid.end_row();
                    grid.label("entry scene");
                    grid.add(
                        egui::TextEdit::singleline(&mut state.entry_text)
                            .desired_width(180.0),
                    );
                    grid.end_row();
                    grid.label("window");
                    grid.horizontal(|row| {
                        row.add(
                            egui::DragValue::new(&mut open.window.width)
                                .range(320..=7680)
                                .suffix("w"),
                        );
                        row.add(
                            egui::DragValue::new(&mut open.window.height)
                                .range(240..=4320)
                                .suffix("h"),
                        );
                    });
                    grid.end_row();
                });
            panel.separator();
            if panel.button("Save project.json.hyg").clicked() {
                save = true;
            }
        });
    if save {
        open.entry = {
            let text = state.entry_text.trim().to_string();
            (!text.is_empty()).then_some(text)
        };
        let dir = std::path::PathBuf::from(&state.scene_path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        match open.write(&dir) {
            Ok(path) => {
                state.set_status(format!("project saved: {}", path.display()));
                state.flash_beam(style::BeamMode::Saved, std::time::Duration::from_millis(1200));
            }
            Err(e) => state.set_status(format!("project save failed: {e}")),
        }
        shared.touch();
    }
    let _ = engine;
}

/// Poll the in-flight dialog; on completion, run its flow. Returns
/// while it stays open (harmless — the frame just re-polls).
fn poll_file_dialog(state: &mut EditorState, engine: &mut Engine, shared: &ShellShared) {    let Some(task) = state.files.as_ref() else {
        return;
    };
    if !task.done.load(Ordering::SeqCst) {
        return;
    }
    let task = state.files.take().expect("checked done");
    let picked = task.result.lock().unwrap().take().flatten();
    match picked {
        Some(path) if task.purpose == DialogPurpose::ModelImport => {
            match import_model(state, engine, &path) {
                Ok(name) => state.set_status(format!("imported {name} — it renders at the orbit target")),
                Err(e) => state.set_status(format!("import failed: {e}")),
            }
            shared.touch();
        }
        Some(path) if task.purpose == DialogPurpose::SceneSave => {
            // the carrier owns the suffix rule — repair what the OS
            // dialog let through
            let name = ensure_hyg_suffix(&path.to_string_lossy());
            let path = path.with_file_name(name);
            match save_scene(&path, &state.scene_camera, engine) {
                Ok(()) => {
                    state.scene_path = path.to_string_lossy().to_string();
                    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                        tab.path = Some(path.clone());
                        tab.label = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("scene")
                            .trim_end_matches(".json")
                            .to_string();
                        tab.dirty = false;
                    }
                    state.set_status(format!("saved {}", path.to_string_lossy()));
                    // the beam HUMS GREEN: a save landed
                    state.flash_beam(style::BeamMode::Saved, std::time::Duration::from_millis(1500));
                }
                Err(e) => state.set_status(format!("save failed: {e}")),
            }
        }
        Some(path) => {
            open_scene_path(state, engine, &path, shared);
            state.set_status(format!("loaded {}", path.to_string_lossy()));
        }
        None => state.set_status("cancelled"),
    }
}

/// The history panel (Godot-style): the command timeline, newest last;
/// applied rows solid, un-done rows dim, the cursor row gold. Clicking a
/// row travels the timeline there (undo/redo as needed).
fn draw_history_window(
    ctx: &egui::Context,
    state: &mut EditorState,
    engine: &mut Engine,
    shared: &ShellShared,
) {
    let mut show = state.show_history;
    let result = egui::Window::new("History")
        .open(&mut show)
        .resizable(true)
        .default_size([340.0, 420.0])
        .min_size([240.0, 180.0])
        .show(ctx, |win| {
            let entries = state.ui.undo_stack().history(engine);
            if entries.is_empty() {
                win.label("no edits yet");
                return;
            }
            let cursor = state.ui.undo_stack().cursor();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(win, |scroll| {
                    for entry in &entries {
                        let at_cursor = entry.timeline_index == cursor;
                        let text = if at_cursor {
                            egui::RichText::new(format!("> {}", entry.label))
                                .strong()
                                .color(style::dark::GOLD)
                        } else if entry.done {
                            egui::RichText::new(&entry.label)
                        } else {
                            egui::RichText::new(&entry.label).weak()
                        };
                        if scroll.selectable_label(at_cursor, text).clicked() {
                            state
                                .ui
                                .undo_stack_mut()
                                .goto(entry.timeline_index, engine);
                            state.set_status(format!("history: back to \"{}\"", entry.label));
                            mark_tab_dirty(state);
                            shared.touch();
                        }
                    }
                });
        });
    state.show_history = show;
    let _ = result;
}

