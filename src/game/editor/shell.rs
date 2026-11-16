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
use crate::game::editor::gizmo::{self, GizmoHit, GizmoLayout, GizmoMode};
use crate::game::editor::pick::{self, Viewport};
use crate::game::editor::style;
use crate::game::editor::ui::EditorUi;
use crate::game::engine::Engine;
use crate::game::render::FrameSink;
use crate::game::scene::Scene;

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
    let pump = {
        let engine = Arc::clone(&engine);
        let pumping = Arc::clone(&pumping);
        let playing = Arc::clone(&playing);
        std::thread::Builder::new()
            .name("hyg-studio-pump".to_string())
            .spawn(move || {
                while pumping.load(Ordering::SeqCst) {
                    let started = Instant::now();
                    if let Ok(mut engine) = engine.lock() {
                        if playing.load(Ordering::SeqCst) {
                            engine.update(1.0 / 60.0);
                        }
                        engine.draw();
                        engine.render();
                    }
                    // pace to ~60 fps of pump work; the display loop
                    // presents at its own cadence
                    let spent = started.elapsed();
                    std::thread::sleep(Duration::from_millis(16).saturating_sub(spent));
                }
            })
            .map_err(|e| format!("studio pump thread: {e}"))?
    };

    let result = run_shell(engine, sink, title.into(), playing);
    pumping.store(false, Ordering::SeqCst);
    let _ = pump.join();
    result
}

/// The winit event loop + egui + blit, on the current thread.
fn run_shell(
    engine: Arc<Mutex<Engine>>,
    sink: FrameSink,
    title: String,
    playing: Arc<AtomicBool>,
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
    let mut handler = ShellHandler {
        engine,
        sink,
        title,
        playing,
        state: EditorState::new(),
        window: None,
        view: None,
        egui_ctx: None,
        egui_state: None,
    };
    event_loop
        .run_app(&mut handler)
        .map_err(|e| format!("studio: event loop error: {e}"))
}

/// Internal message through the event-loop proxy (the M3 surface: pump
/// control messages ride winit's user events; nothing needs them yet).
enum ShellMessage {}

/// The running shell: the wgpu view, the egui halves, and the editor
/// state.
struct ShellHandler {
    engine: Arc<Mutex<Engine>>,
    sink: FrameSink,
    title: String,
    /// The pump's play gate (edit mode = paused; the UI toggles it).
    playing: Arc<AtomicBool>,
    state: EditorState,
    window: Option<Arc<winit::window::Window>>,
    view: Option<ShellView>,
    egui_ctx: Option<egui::Context>,
    egui_state: Option<egui_winit::State>,
}

/// The editor-side state (everything that is not a wgpu resource).
struct EditorState {
    ui: EditorUi,
    mode: GizmoMode,
    drag: Option<DragState>,
    /// What the PRESS landed on (a gizmo handle claims the drag; empty
    /// space claims the pick on release — the two are decided at press).
    press_hit: Option<GizmoHit>,
    scene_path: String,
    dark: bool,
    /// The status line the bottom bar shows (save/load results, hints).
    status: String,
    fps_frames: u32,
    fps_since: Instant,
    fps: u32,
    /// Physical-pixel rect the engine frame letterboxes into (the
    /// picking/gizmo viewport).
    viewport: Option<Viewport>,
    /// The rename field's live buffer (Some while the text edit is
    /// focused — rebuilt-from-entity each frame would eat every
    /// keystroke).
    name_edit: Option<String>,
    /// The history panel's open flag.
    show_history: bool,
    /// The file dialog (None until first opened).
    files: Option<FileDialog>,
}

/// The in-app file dialog (the Godot way — themable, no toolkit deps):
/// a directory browser over `.hyg` documents with save/load modes.
struct FileDialog {
    save: bool,
    dir: std::path::PathBuf,
    /// The save-mode name field.
    name: String,
    /// The load-mode selected file.
    selected: Option<std::path::PathBuf>,
    error: Option<String>,
}

impl EditorState {
    fn new() -> EditorState {
        EditorState {
            ui: EditorUi::new(),
            mode: GizmoMode::Translate,
            drag: None,
            press_hit: None,
            scene_path: "scene.json.hyg".to_string(),
            dark: true,
            status: "click the viewport to pick · W/E/R move/rotate/scale · Ctrl+Z undo · Del deletes"
                .to_string(),
            fps_frames: 0,
            fps_since: Instant::now(),
            fps: 0,
            viewport: None,
            name_edit: None,
            show_history: false,
            files: None,
        }
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
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
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
            }
            winit::event::WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
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

        // egui pass: panels over the live engine (the lock is held while
        // the widgets read/write; the pump waits its turn — a value drag
        // costs a pump frame, nothing worse)
        let raw_input = egui_state.take_egui_input(window);
        egui_ctx.begin_pass(raw_input);
        {
            let mut engine = self.engine.lock().expect("studio engine");
            let frame_dims = view.frame_dims;
            let playing = Arc::clone(&self.playing);
            draw_ui(egui_ctx, &mut self.state, &mut engine, frame_dims, &playing);
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
        let primitives = egui_ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [view.blit.config.width, view.blit.config.height],
            pixels_per_point: output.pixels_per_point,
        };

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

/// Everything egui — one `root_ui` pass: hierarchy left, inspector right,
/// status bottom, and the transparent viewport LAST (the panels carve the
/// sides; what remains is the scene's window). The engine lock is held
/// across the whole pass; `frame_dims` letterboxes the pick viewport.
fn draw_ui(
    ctx: &egui::Context,
    state: &mut EditorState,
    engine: &mut Engine,
    frame_dims: [u32; 2],
    playing: &AtomicBool,
) {
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
            state.status = "undo".to_string();
        }
        if redo_hit && state.ui.redo(engine) {
            state.status = "redo".to_string();
        }
        if let Some(mode) = mode_key {
            state.mode = mode;
        }
        if delete_hit && state.ui.delete_selected(engine, state.now()) {
            state.status = "deleted (Ctrl+Z restores)".to_string();
        }
    }
    // Space toggles edit/play (guarded above — never while typing)
    let space_hit = !ctx.egui_wants_keyboard_input()
        && ctx.input(|input| input.key_pressed(egui::Key::Space) && !ctx.is_pointer_over_egui());
    if space_hit {
        playing.store(!playing.load(Ordering::SeqCst), Ordering::SeqCst);
    }

    let ppp = ctx.pixels_per_point();
    ctx.root_ui(|root| {
        egui::containers::Panel::left("studio-hierarchy")
            .default_size(240.0)
            .show(root, |panel| {
                panel.heading("Hierarchy");
                let nodes = EditorUi::hierarchy(engine);
                egui::ScrollArea::vertical().show(panel, |scroll| {
                    for node in &nodes {
                        hierarchy_rows(scroll, node, state, 0);
                    }
                });
            });

        egui::containers::Panel::right("studio-inspector")
            .default_size(280.0)
            .show(root, |panel| {
                draw_inspector(panel, state, engine);
            });

        egui::containers::Panel::bottom("studio-status")
            .default_size(56.0)
            .show(root, |panel| {
                draw_bottom_bar(panel, state, engine, ctx, playing);
            });

        // the viewport: LAST, frameless so the blit shows through
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(root, |vp| {
                draw_viewport(vp, state, engine, ctx, frame_dims, ppp);
            });
    });

    // floating windows (history, files) live on the context, after panels
    if state.show_history {
        draw_history_window(ctx, state, engine);
    }
    if let Some(mut dialog) = state.files.take() {
        let keep = draw_file_dialog(ctx, state, engine, &mut dialog);
        if keep {
            state.files = Some(dialog);
        }
    }
}

/// The letterboxed pick viewport inside a central rect: the frame fills
/// one axis (the blit's fit rule — [`fit_extents`](crate::game::fit_extents)),
/// centered on the other.
#[must_use]
fn viewport_rect(central: [f32; 2], frame_dims: [u32; 2]) -> Viewport {
    let (cw, ch) = (central[0].max(1.0), central[1].max(1.0));
    let (iw, ih) = (frame_dims[0].max(1) as f32, frame_dims[1].max(1) as f32);
    let scale = if iw / ih > cw / ch { cw / iw } else { ch / ih };
    let (bw, bh) = (iw * scale, ih * scale);
    Viewport::new((cw - bw) * 0.5, (ch - bh) * 0.5, bw, bh)
}

/// The viewport pass: picking, gizmo interaction, gizmo painting. All
/// math in physical pixels; egui's crossing points convert by `ppp`.
fn draw_viewport(
    vp: &mut egui::Ui,
    state: &mut EditorState,
    engine: &mut Engine,
    _ctx: &egui::Context,
    frame_dims: [u32; 2],
    ppp: f32,
) {
    let response = vp.allocate_rect(vp.max_rect(), egui::Sense::click_and_drag());
    let max = vp.max_rect();
    let central = Viewport::new(
        max.min.x * ppp,
        max.min.y * ppp,
        max.width() * ppp,
        max.height() * ppp,
    );
    let vp_rect = viewport_rect([central.width, central.height], frame_dims);
    // the Viewport::new positions are relative; anchor them to the window
    let vp_rect = Viewport::new(
        central.x + vp_rect.x,
        central.y + vp_rect.y,
        vp_rect.width,
        vp_rect.height,
    );
    state.viewport = Some(vp_rect);

    let camera = *engine.camera();

    // the gizmo layout for the current selection (recomputed per frame —
    // the entity moves under the drag)
    let layout: Option<GizmoLayout> = state.ui.selection().and_then(|path| {
        let world = engine.world_transform_at(path)?;
        gizmo::layout(state.mode, &camera, &vp_rect, world.position, world.rotation)
    });

    let pointer_phys = response
        .interact_pointer_pos()
        .map(|p| [p.x * ppp, p.y * ppp]);
    let drag_px = response.drag_delta();
    let drag_phys = [drag_px.x * ppp, drag_px.y * ppp];

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
                    if let (Some(layout), Some(entity)) =
                        (layout.as_ref(), state.ui.selected_entity(engine).map(|e| e.transform().position))
                    {
                        if let Some(handle) = layout.axes.iter().find(|h| h.axis == *axis) {
                            let delta = gizmo::translate_delta(handle, drag_phys);
                            state.ui.set_selected_position(
                                engine,
                                entity + delta,
                                state.now(),
                            );
                        }
                    }
                }
                DragState::TranslateCenter => {
                    if let Some(entity) =
                        state.ui.selected_entity(engine).map(|e| e.transform().position)
                    {
                        let delta =
                            gizmo::translate_center_delta(&camera, &vp_rect, entity, drag_phys);
                        state
                            .ui
                            .set_selected_position(engine, entity + delta, state.now());
                    }
                }
                DragState::ScaleAxis { axis } => {
                    if let (Some(layout), Some(scale)) =
                        (layout.as_ref(), state.ui.selected_entity(engine).map(|e| e.transform().scale))
                    {
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
                        let (_, _, forward_up) = gizmo::view_basis(&camera);
                        // view forward: the basis's up is derived from it;
                        // the ring rotates around the camera's forward —
                        // the axis pointing from the scene toward the eye
                        let (forward, _, _) = gizmo::view_basis(&camera);
                        let _ = forward_up;
                        let delta = gizmo::rotate_delta(d, forward);
                        state.ui.set_selected_rotation(
                            engine,
                            delta.compose(rotation),
                            state.now(),
                        );
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
                    let name = engine
                        .entity_at(&path)
                        .map(|e| e.name().to_string())
                        .unwrap_or_default();
                    state.ui.select(path);
                    state.status = format!("selected {name}");
                }
                None => {
                    state.ui.deselect();
                    state.status = "deselected".to_string();
                }
            }
        }
    }

    // paint the gizmo (points = physical ÷ ppp; the painter clips to the
    // viewport ui)
    if let Some(layout) = &layout {
        paint_gizmo(vp.painter().clone(), layout, state.mode, ppp, state.drag.is_some());
    }
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
        egui::Color32::from_rgb(232, 85, 95),  // x
        egui::Color32::from_rgb(105, 212, 107), // y
        egui::Color32::from_rgb(91, 141, 240), // z
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

/// The hierarchy rows: indent, ⊘ for disabled, component tags, click to
/// select, the selection in gold.
fn hierarchy_rows(
    panel: &mut egui::Ui,
    node: &crate::game::editor::ui::HierarchyNode,
    state: &mut EditorState,
    depth: usize,
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
    // selectable_label: a proper row widget — no text-selection gesture
    // (the old clickable Label left the pointer dragging out highlights)
    let response = panel.selectable_label(selected, label);
    if response.clicked() {
        state.ui.select(node.path.clone());
        state.name_edit = None;
        state.status = format!("selected {}", node.name);
    }
    for child in &node.children {
        hierarchy_rows(panel, child, state, depth + 1);
    }
}

/// The inspector: name, enabled, transform (the gizmo's field-level
/// twin), components with their params, delete. Every write is an undo
/// step. Scrolled — the transform rows plus components overrun the panel
/// on small windows.
fn draw_inspector(panel: &mut egui::Ui, state: &mut EditorState, engine: &mut Engine) {
    panel.heading("Inspector");
    let Some(path) = state.ui.selection().map(|p| p.to_vec()) else {
        state.name_edit = None;
        panel.label("nothing selected — click the viewport or the hierarchy");
        return;
    };
    let Some(entity) = engine.entity_at(&path) else {
        state.ui.deselect();
        state.name_edit = None;
        panel.label("(stale selection — the tree moved)");
        return;
    };
    let name = entity.name().to_string();
    let enabled = entity.enabled();
    let transform = *entity.transform();

    egui::ScrollArea::vertical()
        .id_salt("studio-inspector-scroll")
        .auto_shrink([false, false])
        .show(panel, |scroll| {
            // rename: the buffer persists in the editor state while the
            // field is focused (rebuilt-from-entity each frame would eat
            // every keystroke); committed on blur or Enter
            let mut buf = state.name_edit.clone().unwrap_or_else(|| name.clone());
            let edit = scroll.add(
                egui::TextEdit::singleline(&mut buf).desired_width(f32::INFINITY),
            );
            if edit.has_focus() {
                state.name_edit = Some(buf.clone());
            }
            if edit.lost_focus() {
                state.name_edit = None;
                let trimmed = buf.trim().to_string();
                if !trimmed.is_empty() && trimmed != name {
                    state.ui.rename_selected(engine, &trimmed, state.now());
                }
            }
            let mut enabled_buf = enabled;
            if scroll.checkbox(&mut enabled_buf, "enabled").changed() {
                state.ui.set_selected_enabled(engine, enabled_buf, state.now());
            }

            scroll.separator();
            scroll.strong("transform");
            let mut position = transform.position;
            let mut moved = false;
            scroll.label("position");
            for (axis, value) in [
                ("x", &mut position.x),
                ("y", &mut position.y),
                ("z", &mut position.z),
            ] {
                let mut v = f64::from(*value);
                if scroll
                    .add(egui::DragValue::new(&mut v).speed(0.05).prefix(format!("{axis} ")))
                    .changed()
                {
                    *value = v as f32;
                    moved = true;
                }
            }
            if moved {
                state.ui.set_selected_position(engine, position, state.now());
            }

            let mut scale = transform.scale;
            let mut scaled = false;
            scroll.label("scale");
            for (axis, value) in [
                ("x", &mut scale.x),
                ("y", &mut scale.y),
                ("z", &mut scale.z),
            ] {
                let mut v = f64::from(*value);
                if scroll
                    .add(egui::DragValue::new(&mut v).speed(0.05).prefix(format!("{axis} ")))
                    .changed()
                {
                    *value = v as f32;
                    scaled = true;
                }
            }
            if scaled {
                state.ui.set_selected_scale(engine, scale, state.now());
            }

            scroll.label("rotation (quaternion)");
            let mut rotation = transform.rotation;
            let mut rotated = false;
            for (axis, value) in [
                ("x", &mut rotation.x),
                ("y", &mut rotation.y),
                ("z", &mut rotation.z),
                ("w", &mut rotation.w),
            ] {
                let mut v = f64::from(*value);
                if scroll
                    .add(egui::DragValue::new(&mut v).speed(0.01).prefix(format!("{axis} ")))
                    .changed()
                {
                    *value = v as f32;
                    rotated = true;
                }
            }
            if rotated {
                state.ui.set_selected_rotation(engine, rotation, state.now());
            }

            scroll.separator();
            scroll.strong("components");
            if let Some(entity) = engine.entity_at(&path) {
                let mut any = false;
                for component in entity.iter_components() {
                    any = true;
                    let type_name = component.scene_type().unwrap_or("(runtime only)");
                    let ctx = crate::game::scene::ComponentContext::new(entity.name());
                    let params = component.scene_params(&ctx);
                    let detail = match &params {
                        Some(var) => match var.as_object() {
                            Some(object) if !object.is_empty() => {
                                let keys: Vec<String> =
                                    object.entries().iter().map(|(k, _)| k.clone()).collect();
                                format!("  · {type_name}  [{}]", keys.join(", "))
                            }
                            _ => format!("  · {type_name}  (defaults)"),
                        },
                        None => format!("  · {type_name}"),
                    };
                    scroll.label(detail);
                }
                if !any {
                    scroll.label("  (none)");
                }
            }

            scroll.separator();
            if scroll
                .add(egui::Button::new("delete").fill(style::dark::DANGER))
                .clicked()
                && state.ui.delete_selected(engine, state.now())
            {
                state.status = "deleted (Ctrl+Z restores)".to_string();
            }
        });
}

/// The bottom bar: play/pause, undo/redo, history, gizmo modes, scene
/// save/load (the file dialog), theme, fps — the editor's cockpit. All
/// ASCII labels: the default font carries no ↶/☀/☿ glyphs, and tofu
/// boxes read as bugs.
fn draw_bottom_bar(
    panel: &mut egui::Ui,
    state: &mut EditorState,
    engine: &mut Engine,
    ctx: &egui::Context,
    playing: &AtomicBool,
) {
    panel.horizontal(|bar| {
        // play/pause — the edit-mode gate (paused default)
        let playing_now = playing.load(Ordering::SeqCst);
        let play_label = if playing_now { "pause" } else { "play" };
        if bar.button(play_label).clicked() {
            playing.store(!playing_now, Ordering::SeqCst);
            state.status = if playing_now {
                "edit mode — the sim is paused, edits stick".to_string()
            } else {
                "playing — press Space or pause to edit".to_string()
            };
        }

        bar.separator();
        if bar
            .add_enabled(state.ui.undo_stack().can_undo(), egui::Button::new("undo"))
            .clicked()
            && state.ui.undo(engine)
        {
            state.status = "undo".to_string();
        }
        if bar
            .add_enabled(state.ui.undo_stack().can_redo(), egui::Button::new("redo"))
            .clicked()
            && state.ui.redo(engine)
        {
            state.status = "redo".to_string();
        }
        if bar.button("history").clicked() {
            state.show_history = !state.show_history;
        }

        bar.separator();
        for mode in [GizmoMode::Translate, GizmoMode::Rotate, GizmoMode::Scale] {
            let label = egui::RichText::new(mode.name());
            let label = if state.mode == mode {
                label.strong().color(style::dark::GOLD)
            } else {
                label
            };
            if bar.button(label).clicked() {
                state.mode = mode;
            }
        }

        bar.separator();
        if bar.button("save...").clicked() {
            let name = state
                .scene_path
                .rsplit('/')
                .next()
                .unwrap_or("scene.json.hyg")
                .to_string();
            state.files = Some(FileDialog {
                save: true,
                dir: std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf()),
                name,
                selected: None,
                error: None,
            });
        }
        if bar.button("load...").clicked() {
            state.files = Some(FileDialog {
                save: false,
                dir: std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf()),
                name: String::new(),
                selected: None,
                error: None,
            });
        }
        let short = state
            .scene_path
            .rsplit('/')
            .next()
            .unwrap_or(&state.scene_path)
            .to_string();
        bar.monospace(format!(" {short}"));

        bar.separator();
        let theme_label = if state.dark { "light" } else { "dark" };
        if bar.button(theme_label).clicked() {
            state.dark = !state.dark;
            style::apply(ctx, state.dark);
        }

        bar.with_layout(egui::Layout::right_to_left(egui::Align::Center), |right| {
            right.label(format!("{} fps", state.fps));
            right.label(&state.status);
        });
    });
}

/// The scene save (path from the dialog): the M0 canonical document of
/// the live tree, the name from the path stem, the format from the
/// filename.
fn save_scene(path: &Path, engine: &Engine) -> Result<(), String> {
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("untitled")
        .trim_end_matches(".json")
        .to_string();
    let doc = Scene::document(&name, engine.camera(), engine.entities());
    carrier::write(path, &doc).map_err(|e| e.to_string())
}

/// The scene load (path from the dialog): the carrier reads, the scene
/// materializes, the engine replaces its roots and camera. The undo
/// stack clears (stored paths moved with the old tree) and the selection
/// resets.
fn load_scene(state: &mut EditorState, engine: &mut Engine) -> Result<(), String> {
    let scene = Scene::load(Path::new(&state.scene_path)).map_err(|e| e.to_string())?;
    engine.set_camera(scene.camera);
    engine.set_roots(scene.entities);
    state.ui = EditorUi::new();
    state.drag = None;
    state.name_edit = None;
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

/// The history panel (Godot-style): the command timeline, newest last;
/// applied rows solid, un-done rows dim, the cursor row gold. Clicking a
/// row travels the timeline there (undo/redo as needed).
fn draw_history_window(ctx: &egui::Context, state: &mut EditorState, engine: &mut Engine) {
    egui::Window::new("History")
        .open(&mut state.show_history)
        .default_width(280.0)
        .show(ctx, |win| {
            let entries = state.ui.undo_stack().history(engine);
            if entries.is_empty() {
                win.label("no edits yet");
                return;
            }
            let cursor = state.ui.undo_stack().cursor();
            egui::ScrollArea::vertical()
                .max_height(320.0)
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
                            state.status = format!("history: back to \"{}\"", entry.label);
                        }
                    }
                });
        });
}

/// The in-app file dialog (the Godot way — themable, no toolkit deps):
/// a directory browser over `.hyg` documents. Save mode has a name
/// field (the suffix auto-repairs); Load mode selects a file. ".."
/// climbs; double-clicking a directory enters it.
fn draw_file_dialog(
    ctx: &egui::Context,
    state: &mut EditorState,
    engine: &mut Engine,
    dialog: &mut FileDialog,
) -> bool {
    // returns whether the dialog stays open (false = confirmed/cancelled);
    // `confirmed` crosses the egui closures (a `return` would only exit
    // the closure)
    let mut open = true;
    let mut confirmed = false;
    egui::Window::new(if dialog.save { "Save scene" } else { "Load scene" })
        .open(&mut open)
        .default_width(440.0)
        .show(ctx, |win| {
            // the current directory, clickable parent segments
            win.horizontal(|row| {
                if row.small_button("up").clicked() {
                    if let Some(parent) = dialog.dir.parent() {
                        dialog.dir = parent.to_path_buf();
                    }
                    dialog.selected = None;
                    dialog.error = None;
                }
                row.monospace(display_dir(&dialog.dir));
            });
            win.separator();

            // entries: directories first, then .hyg files, both sorted
            let mut dirs: Vec<String> = Vec::new();
            let mut files: Vec<String> = Vec::new();
            if let Ok(read) = std::fs::read_dir(&dialog.dir) {
                for entry in read.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    if is_dir {
                        if !name.starts_with('.') {
                            dirs.push(format!("{name}/"));
                        }
                    } else if name.ends_with(".hyg") {
                        files.push(name);
                    }
                }
            }
            dirs.sort();
            files.sort();

            let mut entered: Option<String> = None;
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(win, |scroll| {
                    for name in dirs.iter().chain(files.iter()) {
                        let is_file = !name.ends_with('/');
                        let selected = dialog
                            .selected
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy() == *name)
                            .unwrap_or(false);
                        let response = scroll.selectable_label(selected, name);
                        if response.clicked() || response.double_clicked() {
                            if is_file {
                                dialog.selected =
                                    Some(dialog.dir.join(name.trim_end_matches('/')));
                                if dialog.save && response.double_clicked() {
                                    dialog.name =
                                        name.trim_end_matches('/').to_string();
                                }
                            } else if response.double_clicked()
                                || response.clicked() && ctx.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                entered = Some(name.trim_end_matches('/').to_string());
                            }
                        }
                    }
                });
            if let Some(dir) = entered {
                dialog.dir = dialog.dir.join(dir);
                dialog.selected = None;
                dialog.error = None;
            }

            win.separator();
            if dialog.save {
                win.horizontal(|row| {
                    row.label("name");
                    row.text_edit_singleline(&mut dialog.name);
                });
            }
            if let Some(error) = &dialog.error {
                win.colored_label(style::dark::DANGER, error.clone());
            }

            win.horizontal(|row| {
                let confirm_label = if dialog.save { "save" } else { "load" };
                if row.button(confirm_label).clicked() {
                    if dialog.save {
                        let trimmed = dialog.name.trim().to_string();
                        if trimmed.is_empty() {
                            dialog.error = Some("name the scene first".to_string());
                        } else {
                            let path = dialog.dir.join(ensure_hyg_suffix(&trimmed));
                            match save_scene(&path, engine) {
                                Ok(()) => {
                                    state.scene_path = path.to_string_lossy().to_string();
                                    state.status =
                                        format!("saved {}", path.to_string_lossy());
                                    confirmed = true;
                                }
                                Err(e) => dialog.error = Some(e),
                            }
                        }
                    } else {
                        match dialog.selected.clone() {
                            Some(path) => {
                                state.scene_path = path.to_string_lossy().to_string();
                                match load_scene(state, engine) {
                                    Ok(()) => {
                                        state.status = format!(
                                            "loaded {}",
                                            path.to_string_lossy()
                                        );
                                        confirmed = true;
                                    }
                                    Err(e) => dialog.error = Some(e),
                                }
                            }
                            None => {
                                dialog.error = Some("pick a .hyg scene".to_string());
                            }
                        }
                    }
                }
                if row.button("cancel").clicked() {
                    confirmed = true;
                }
                true
            });
        });
    open && !confirmed
}

/// A short, readable directory label (home → ~).
fn display_dir(dir: &Path) -> String {
    let text = dir.to_string_lossy().to_string();
    if let Some(home) = std::env::home_dir() {
        let home = home.to_string_lossy().to_string();
        if text == home {
            return "~".to_string();
        }
        if let Some(rest) = text.strip_prefix(&format!("{home}/")) {
            return format!("~/{rest}");
        }
    }
    text
}
