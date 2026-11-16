//! The Studio shell — a winit window with the engine pump on another
//! thread, the engine's frame blitted under egui panels on ONE surface.
//! The layering the design's open question asked to verify, as code:
//! winit 0.30 events feed egui-winit, the engine frame is the underlay
//! pass (the native view's blit, shared), and egui composites on top in a
//! second load pass of the same frame.
//!
//! Threading mirrors the native view's shape, roles flipped: the editor is
//! the app, so winit owns the MAIN thread (no macOS refusal — the pump is
//! the portable worker half), and the pump drives the
//! [`Engine`](crate::game::Engine) through a shared mutex. The panels
//! read/write the live tree under that lock between pump frames — a value
//! drag can cost the pump a frame; correctness first, the pump otherwise
//! runs at its own cadence. This is also the debugger's renderer rule as
//! built: a paused graph stops ADVANCING (the pump skips its drive), but
//! this loop keeps drawing — the editor shows live panels over the last
//! engine frame, never a dead window.
//!
//! Headless-first: none of this runs in tests (windowing is exercised by
//! hand); the testable half is [`super::ui`](super::ui), and the pure
//! helpers ([`fit_extents`](crate::game::fit_extents),
//! [`should_present`](crate::game::should_present)) carry their own tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::game::editor::ui::EditorUi;
use crate::game::engine::Engine;
use crate::game::render::FrameSink;

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
    let pump = {
        let engine = Arc::clone(&engine);
        let pumping = Arc::clone(&pumping);
        std::thread::Builder::new()
            .name("hyg-studio-pump".to_string())
            .spawn(move || {
                while pumping.load(Ordering::SeqCst) {
                    let started = Instant::now();
                    if let Ok(mut engine) = engine.lock() {
                        engine.update(1.0 / 60.0);
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

    let result = run_shell(engine, sink, title.into());
    pumping.store(false, Ordering::SeqCst);
    let _ = pump.join();
    result
}

/// The winit event loop + egui + blit, on the current thread.
fn run_shell(engine: Arc<Mutex<Engine>>, sink: FrameSink, title: String) -> Result<(), String> {
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
        ui: EditorUi::new(),
        window: None,
        view: None,
        egui_ctx: None,
        egui_state: None,
    };
    event_loop
        .run_app(&mut handler)
        .map_err(|e| format!("studio: event loop error: {e}"))
}

/// Internal message through the event-loop proxy (the M2 surface: gizmo
/// and tool messages ride winit's user events; nothing needs them yet).
enum ShellMessage {}

/// The running shell's per-window state.
struct ShellHandler {
    engine: Arc<Mutex<Engine>>,
    sink: FrameSink,
    title: String,
    ui: EditorUi,
    window: Option<Arc<winit::window::Window>>,
    view: Option<ShellView>,
    egui_ctx: Option<egui::Context>,
    egui_state: Option<egui_winit::State>,
}

/// Per-window wgpu state: the engine-frame blit (the underlay pass, the
/// native view's shared machinery) plus the egui renderer (the overlay).
struct ShellView {
    blit: crate::game::native_view::BlitView,
    egui_renderer: egui_wgpu::Renderer,
    /// The last-uploaded sink generation (unchanged engine frames skip the
    /// upload; egui repaints regardless — the panels are live).
    seen: u64,
}

impl ShellView {
    fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        self.blit.resize(size);
    }
}

#[cfg(feature = "editor")]
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
        let view = match pollster::block_on(crate::game::native_view::build_blit_view(&window)) {
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
                }
            }
            Err(e) => {
                eprintln!("studio: {e} — closing");
                event_loop.exit();
                return;
            }
        };
        let egui_ctx = egui::Context::default();
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
            "studio: \"{}\" — hierarchy left, inspector right; close the window (or the panel's ✕) to quit",
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

        // egui pass: panels over the live engine (the lock is held while
        // the widgets read/write; the pump waits its turn — a value drag
        // costs a pump frame, nothing worse)
        let raw_input = egui_state.take_egui_input(window);
        egui_ctx.begin_pass(raw_input);
        {
            let mut engine = self.engine.lock().expect("studio engine");
            draw_panels(egui_ctx, &mut self.ui, &mut engine);
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

/// The panels: hierarchy left, inspector right, status bottom — all reads
/// and writes through [`EditorUi`](crate::game::editor::ui::EditorUi) and
/// the engine's public surface.
fn draw_panels(ctx: &egui::Context, ui: &mut EditorUi, engine: &mut Engine) {
    ctx.root_ui(|root| {
        egui::containers::Panel::left("studio-hierarchy")
            .default_size(240.0)
            .show(root, |panel| {
                panel.heading("Hierarchy");
                let nodes = EditorUi::hierarchy(engine);
                egui::ScrollArea::vertical().show(panel, |scroll| {
                    for node in &nodes {
                        hierarchy_rows(scroll, node, ui, 0);
                    }
                });
            });

        egui::containers::Panel::right("studio-inspector")
            .default_size(280.0)
            .show(root, |panel| {
                panel.heading("Inspector");
                let Some(path) = ui.selection().map(|p| p.to_vec()) else {
                    panel.label("nothing selected — click an entity in the hierarchy");
                    return;
                };
                let Some(entity) = engine.entity_at(&path) else {
                    ui.deselect();
                    panel.label("(stale selection — the tree moved)");
                    return;
                };
                let name = entity.name().to_string();
                let enabled = entity.enabled();
                let transform = *entity.transform();

                let mut name_buf = name.clone();
                if panel
                    .add(egui::TextEdit::singleline(&mut name_buf).desired_width(f32::INFINITY))
                    .changed()
                    && !name_buf.trim().is_empty()
                    && name_buf != name
                {
                    ui.rename_selected(engine, name_buf.trim());
                }
                let mut enabled_buf = enabled;
                if panel.checkbox(&mut enabled_buf, "enabled").changed() {
                    ui.set_selected_enabled(engine, enabled_buf);
                }

                panel.separator();
                panel.strong("transform");
                panel.label("position");
                let mut position = transform.position;
                let mut moved = false;
                for (axis, value) in [("x", &mut position.x), ("y", &mut position.y), ("z", &mut position.z)] {
                    let mut v = f64::from(*value);
                    if panel
                        .add(egui::DragValue::new(&mut v).speed(0.05).prefix(format!("{axis} ")))
                        .changed()
                    {
                        *value = v as f32;
                        moved = true;
                    }
                }
                if moved {
                    ui.set_selected_position(engine, position);
                }

                panel.label("scale");
                let mut scale = transform.scale;
                let mut scaled = false;
                for (axis, value) in [("x", &mut scale.x), ("y", &mut scale.y), ("z", &mut scale.z)] {
                    let mut v = f64::from(*value);
                    if panel
                        .add(egui::DragValue::new(&mut v).speed(0.05).prefix(format!("{axis} ")))
                        .changed()
                    {
                        *value = v as f32;
                        scaled = true;
                    }
                }
                if scaled {
                    ui.set_selected_scale(engine, scale);
                }

                panel.label("rotation (quaternion)");
                let mut rotation = transform.rotation;
                let mut rotated = false;
                for (axis, value) in [
                    ("x", &mut rotation.x),
                    ("y", &mut rotation.y),
                    ("z", &mut rotation.z),
                    ("w", &mut rotation.w),
                ] {
                    let mut v = f64::from(*value);
                    if panel
                        .add(egui::DragValue::new(&mut v).speed(0.01).prefix(format!("{axis} ")))
                        .changed()
                    {
                        *value = v as f32;
                        rotated = true;
                    }
                }
                if rotated {
                    ui.set_selected_rotation(engine, rotation);
                }

                panel.separator();
                let components = component_list(&path, engine);
                if !components.is_empty() {
                    panel.label("components:");
                    for line in components {
                        panel.label(format!("  {line}"));
                    }
                }
            });

        egui::containers::Panel::bottom("studio-status").show(root, |panel| {
            panel.label(format!(
                "Hydrargyros Studio ☿ — {} root entities",
                engine.entities().len()
            ));
        });
    });
}

/// The selected entity's component registry names (runtime-only
/// components label themselves — the inspector shows what is there).
fn component_list(path: &[usize], engine: &Engine) -> Vec<String> {
    match engine.entity_at(path) {
        Some(entity) => entity
            .iter_components()
            .map(|c| c.scene_type().unwrap_or("(runtime only)").to_string())
            .collect(),
        None => Vec::new(),
    }
}

/// One hierarchy row per entity, children indented; clicking selects.
fn hierarchy_rows(
    panel: &mut egui::Ui,
    node: &crate::game::editor::ui::HierarchyNode,
    ui: &mut EditorUi,
    depth: usize,
) {
    let selected = ui.selection() == Some(node.path.as_slice());
    let mut label = String::new();
    label.push_str(&"    ".repeat(depth));
    if !node.enabled {
        label.push_str("⊘ ");
    }
    label.push_str(&node.name);
    if !node.component_types.is_empty() {
        label.push_str(&format!("  · {}", node.component_types.join(", ")));
    }
    let text = if selected {
        egui::RichText::new(label)
            .color(egui::Color32::from_rgb(255, 209, 102))
            .strong()
    } else {
        egui::RichText::new(label)
    };
    let response = panel.add(egui::Label::new(text).sense(egui::Sense::click()));
    if response.clicked() {
        ui.select(node.path.clone());
    }
    for child in &node.children {
        hierarchy_rows(panel, child, ui, depth + 1);
    }
}
