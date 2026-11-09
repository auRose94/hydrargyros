//! The native view — a winit window presenting the SAME offscreen frames the
//! stream page shows (the fullscreen hand-off target; the pump never
//! restarts, the buffer is simply consumed by a second presenter).
//!
//! Design: [`NativeView::spawn`] runs a winit event loop on its own thread.
//! The loop owns a mini wgpu blit renderer: the newest [`FrameSink`] frame
//! is uploaded as a texture and drawn over the surface through a bilinear
//! sampler, letterboxed to the image's aspect — the same smoothing the
//! stream page's fit mode uses. Frames are skipped by generation
//! ([`should_present`]), so an unchanged buffer is never re-uploaded, and
//! the loop wakes on a 60 Hz tick instead of spinning. Esc or the window's
//! close button exits THE VIEW only: this loop has no path back into the
//! engine's event queue (no `WindowSystem` access, by design), so it can
//! never quit-inject — the stream keeps running and the page still works.
//!
//! Platform note, honestly: winit's event loop must own the thread it is
//! created on, and 0.30 PANICS when a loop is initialized off-main on
//! Linux unless both backends' `any_thread` escape hatches are set — which
//! `run_view` does (only the active backend reads its own flag), so Linux
//! (X11/Wayland) works from the worker thread. Windows allows off-main
//! event loops natively. macOS has no escape hatch (the main-thread rule
//! `winit_window.rs` documents), so there the view refuses with an honest
//! message instead of panicking — a macOS port would hop to main first.
//!
//! Compiled without the `wgpu` feature, `spawn` returns an honest error and
//! the stream page is unaffected (the view is a second consumer, not a
//! requirement). CI runs no window tests — the pure helpers
//! ([`fit_extents`], [`should_present`]) are extracted and unit-tested
//! instead (tests/stream.rs); the real spawn is exercised by hand.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::game::render::FrameSink;
// `Frame` is only touched by the wgpu presenter (upload/present); a
// winit-only build would read the import as unused
#[cfg(feature = "wgpu")]
use crate::game::render::Frame;

/// The fullscreen hand-off target: spawn with [`NativeView::spawn`], steer
/// with [`NativeViewHandle`].
pub struct NativeView;

impl NativeView {
    /// Spawn the native view thread: a borderless-fullscreen window
    /// presenting the sink's frames. The view is a SECOND consumer of the
    /// render buffer — the pump never restarts, and closing the view (Esc,
    /// the close button, or [`NativeViewHandle::close`]) never touches the
    /// app. The frame at spawn time appears immediately; fresh frames
    /// follow as the engine renders them.
    ///
    /// # Errors
    /// The thread could not spawn — or, compiled without the `wgpu`
    /// feature, the view is unavailable (the stream page still works).
    pub fn spawn(sink: FrameSink, title: impl Into<String>) -> Result<NativeViewHandle, String> {
        #[cfg(not(feature = "wgpu"))]
        {
            let _ = (sink, title);
            Err("native view needs the wgpu feature (rebuild with --features wgpu)".to_string())
        }
        #[cfg(feature = "wgpu")]
        {
            let shared = Arc::new(Shared {
                open: AtomicBool::new(false),
                close_requested: AtomicBool::new(false),
                proxy: Mutex::new(None),
            });
            let thread_shared = Arc::clone(&shared);
            let title = title.into();
            std::thread::Builder::new()
                .name("hyg-native-view".to_string())
                .spawn(move || run_view(sink, title, thread_shared))
                .map_err(|e| format!("native view thread: {e}"))?;
            Ok(NativeViewHandle { shared })
        }
    }
}

/// A live native view: [`is_open`](NativeViewHandle::is_open) is the cheap
/// liveness probe, [`close`](NativeViewHandle::close) the app-side exit.
/// Dropping the handle does NOT close the view — it runs until Esc, the
/// window's close button, or an explicit `close`.
pub struct NativeViewHandle {
    shared: Arc<Shared>,
}

impl NativeViewHandle {
    /// Cheap liveness probe: false once the view's loop has exited (and
    /// always, when compiled without `wgpu`).
    pub fn is_open(&self) -> bool {
        self.shared.open.load(Ordering::SeqCst)
    }

    /// Ask the view to close (safe from any thread). The view exits its own
    /// event loop and prints its close line; nothing else in the app — not
    /// the pump, not the stream page — is touched.
    pub fn close(&self) {
        self.shared.close_requested.store(true, Ordering::SeqCst);
        if let Some(proxy) = self.shared.proxy.lock().unwrap().as_ref() {
            let _ = proxy.send_event(ViewMessage::Close);
        }
    }
}

struct Shared {
    open: AtomicBool,
    close_requested: AtomicBool,
    /// Wakes the loop for `close` (winit's thread-safe user-event path).
    proxy: Mutex<Option<winit::event_loop::EventLoopProxy<ViewMessage>>>,
}

/// Internal message through the event-loop proxy.
enum ViewMessage {
    Close,
}

/// The frame-skip rule: present only when the sink's newest frame is
/// genuinely newer than what's on screen (`seen` is the presented
/// generation) — the loop's "don't spin at 1000 fps on an unchanged frame".
/// Pure and public for the unit tests (tests/stream.rs).
pub fn should_present(seen: u64, candidate: Option<u64>) -> bool {
    candidate.is_some_and(|generation| generation > seen)
}

/// The clip-space half-extents that letterbox an `image` inside a `surface`
/// of different aspect: the image fills ONE axis, the other shows the clear
/// color (black bars), aspect always preserved. Pure and public for the
/// unit tests (tests/stream.rs).
pub fn fit_extents(surface: (u32, u32), image: (u32, u32)) -> [f32; 2] {
    let (sw, sh) = (surface.0.max(1) as f32, surface.1.max(1) as f32);
    let (iw, ih) = (image.0.max(1) as f32, image.1.max(1) as f32);
    let surface_ratio = sw / sh;
    let image_ratio = iw / ih;
    if image_ratio > surface_ratio {
        // image wider than the surface: full width, letterboxed height
        [1.0, surface_ratio / image_ratio]
    } else {
        // image taller (or equal): full height, pillarboxed width
        [image_ratio / surface_ratio, 1.0]
    }
}

/// The blit: a uv-driven quad (six vertices) whose clip rect is the
/// letterbox fit, the frame texture sampled with the bilinear sampler.
#[cfg(feature = "wgpu")]
const BLIT_WGSL: &str = r#"
@group(0) @binding(0) var<uniform> fit: vec4f; // x, y = clip half-extents
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VsOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    var corners = array(
        vec2f(0.0, 1.0), vec2f(0.0, 0.0), vec2f(1.0, 0.0),
        vec2f(0.0, 1.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0),
    );
    let c = corners[i];
    var out: VsOut;
    // orientation: the sink's rgba rows run top-down (row 0 = the image's
    // top, which lands at texture v=0 on upload), while clip +y is the
    // SCREEN's top — so screen-top corners (clip.y > 0) must sample
    // v=0: flip v against the corner's clip y, or the view renders the
    // frame upside down.
    out.uv = vec2f(c.x, 1.0 - c.y);
    out.clip = vec4f((c * 2.0 - 1.0) * fit.xy, 0.0, 1.0);
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
    return vec4f(textureSample(tex, samp, in.uv).rgb, 1.0);
}
"#;

#[cfg(feature = "wgpu")]
fn run_view(sink: FrameSink, title: String, shared: Arc<Shared>) {
    // macOS: winit's event loop must be created on the MAIN thread (no
    // any-thread escape hatch exists there) — refuse honestly instead of
    // panicking on the worker thread (a macOS port would hop to main first).
    #[cfg(target_os = "macos")]
    {
        let _ = (&sink, &title, &shared);
        eprintln!(
            "native view: macOS needs the event loop on the main thread — \
             not supported from spawn yet (the stream page still works)"
        );
        return;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut builder = winit::event_loop::EventLoop::<ViewMessage>::with_user_event();
        // Linux: this is a worker thread, and winit PANICS when a loop is
        // initialized off-main unless the backend's escape hatch is set —
        // set it for both (only the active backend reads its own flag).
        // Windows allows off-main event loops natively; no opt-in needed.
        #[cfg(target_os = "linux")]
        {
            {
                use winit::platform::x11::EventLoopBuilderExtX11;
                builder.with_any_thread(true);
            }
            {
                use winit::platform::wayland::EventLoopBuilderExtWayland;
                builder.with_any_thread(true);
            }
        }
        let event_loop = match builder.build() {
            Ok(loop_) => loop_,
            Err(e) => {
                eprintln!(
                    "native view: event loop: {e} (no display server? the stream page still works)"
                );
                return;
            }
        };
        *shared.proxy.lock().unwrap() = Some(event_loop.create_proxy());
        let mut handler = ViewHandler {
            sink,
            title,
            shared: Arc::clone(&shared),
            view: None,
        };
        // exits on Esc, the close button, or handle.close() — THE VIEW ONLY
        let _ = event_loop.run_app(&mut handler);
        shared.open.store(false, Ordering::SeqCst);
        println!("native view: closed — the stream keeps running");
    }
}

/// One view's renderer: the surface, the blit pipeline, and the uploaded
/// frame texture (recreated when the frame size changes, written on every
/// new generation).
#[cfg(feature = "wgpu")]
struct ViewState {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    bind_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    /// The letterbox fit uniform (two clip-space half-extents).
    fit: wgpu::Buffer,
    frame_tex: wgpu::Texture,
    frame_view: wgpu::TextureView,
    bind: wgpu::BindGroup,
    /// The uploaded frame's dimensions (the fit depends on them).
    image: [u32; 2],
    seen: u64,
}

#[cfg(feature = "wgpu")]
impl ViewState {
    fn present_frame(&mut self, frame: &Frame) {
        self.upload(frame);
        self.seen = frame.generation;
        self.draw();
    }

    /// Upload the frame's rgba as an Rgba8UnormSrgb texture (sRGB bytes in,
    /// sRGB surface out — net identity), recreate it when the size changed,
    /// and re-fit the letterbox.
    fn upload(&mut self, frame: &Frame) {
        let dims = [frame.width, frame.height];
        if self.image != dims {
            self.image = dims;
            let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("hyg-native-view-frame"),
                size: wgpu::Extent3d {
                    width: frame.width,
                    height: frame.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hyg-native-view-bind"),
                layout: &self.bind_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.fit.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.frame_tex = tex;
            self.frame_view = view;
            self.bind = bind;
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.frame_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &frame.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(frame.width * 4),
                rows_per_image: Some(frame.height),
            },
            wgpu::Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
        );
        self.update_fit();
    }

    /// Reconfigure the surface for a new size and re-fit (the fit uniform
    /// depends on both the image and the surface aspect).
    fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        self.config.width = size.width.max(1);
        self.config.height = size.height.max(1);
        self.surface.configure(&self.device, &self.config);
        self.update_fit();
    }

    fn update_fit(&self) {
        let extents = fit_extents(
            (self.config.width, self.config.height),
            (self.image[0], self.image[1]),
        );
        let floats = [extents[0], extents[1], 0.0, 0.0];
        let bytes: Vec<u8> = floats.iter().flat_map(|f| f.to_ne_bytes()).collect();
        self.queue.write_buffer(&self.fit, 0, &bytes);
    }

    /// Clear black (the letterbox bars) and blit the frame texture.
    fn draw(&mut self) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            _ => return, // skip: timeout/occluded/outdated/lost
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hyg-native-view-blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.draw(0..6, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
    }
}

/// Build the wgpu surface + blit pipeline against the created window.
#[cfg(feature = "wgpu")]
async fn build_view(window: &std::sync::Arc<winit::window::Window>) -> Result<ViewState, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let surface = instance
        .create_surface(window.clone())
        .map_err(|e| format!("native view surface: {e}"))?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("native view: no wgpu adapter for the window surface: {e}"))?;
    let caps = surface.get_capabilities(&adapter);
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| f.is_srgb())
        .unwrap_or(caps.formats[0]);
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .map_err(|e| format!("native view: wgpu device: {e}"))?;

    let (width, height) = (
        window.inner_size().width.max(1),
        window.inner_size().height.max(1),
    );
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width,
        height,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        color_space: wgpu::SurfaceColorSpace::Srgb,
    };
    surface.configure(&device, &config);

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("hyg-native-view-blit"),
        source: wgpu::ShaderSource::Wgsl(BLIT_WGSL.into()),
    });
    let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("hyg-native-view-bind"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(16),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&bind_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("hyg-native-view-blit"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        cache: None,
        multiview_mask: None,
    });
    // BILINEAR — the fit upscale smooths, matching the page's fit mode
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("hyg-native-view-sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let fit = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hyg-native-view-fit"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // a 1×1 black frame until the first upload (the bind group is whole)
    let frame_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hyg-native-view-frame"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let frame_view = frame_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("hyg-native-view-bind"),
        layout: &bind_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: fit.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&frame_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    let state = ViewState {
        surface,
        config,
        device,
        queue,
        bind_layout,
        pipeline,
        sampler,
        fit,
        frame_tex,
        frame_view,
        bind,
        image: [1, 1],
        seen: 0,
    };
    state.update_fit();
    Ok(state)
}

#[cfg(feature = "wgpu")]
struct ViewHandler {
    sink: FrameSink,
    title: String,
    shared: Arc<Shared>,
    view: Option<ViewState>,
}

#[cfg(feature = "wgpu")]
impl winit::application::ApplicationHandler<ViewMessage> for ViewHandler {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        // borderless fullscreen on the CURRENT monitor (`None` = winit picks
        // the monitor the window lands on)
        let attributes = winit::window::Window::default_attributes()
            .with_title(self.title.clone())
            .with_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => std::sync::Arc::new(window),
            Err(e) => {
                eprintln!("native view: window creation failed: {e}");
                event_loop.exit();
                return;
            }
        };
        match pollster::block_on(build_view(&window)) {
            Ok(view) => {
                self.view = Some(view);
                self.shared.open.store(true, Ordering::SeqCst);
                println!(
                    "native view: opened \"{}\" — Esc closes the view; the stream page keeps running",
                    self.title
                );
                // wake at the pump's cadence: present new frames, skip
                // unchanged ones (should_present), no busy spin
                event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                    std::time::Instant::now() + std::time::Duration::from_millis(16),
                ));
            }
            Err(e) => {
                eprintln!("native view: {e} — closing (the stream page still works)");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        match event {
            winit::event::WindowEvent::CloseRequested => event_loop.exit(),
            // Esc exits THE VIEW only — no event reaches the engine's queue
            // from here (no WindowSystem access, by design)
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
                if let Some(state) = self.view.as_mut() {
                    state.resize(size);
                    state.draw(); // re-present the current frame on the new surface
                }
            }
            winit::event::WindowEvent::RedrawRequested => {
                if let Some(state) = self.view.as_mut() {
                    state.draw();
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, event: ViewMessage) {
        if matches!(event, ViewMessage::Close) {
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.shared.close_requested.load(Ordering::SeqCst) {
            event_loop.exit();
            return;
        }
        if let Some(state) = self.view.as_mut() {
            if let Some(frame) = self.sink.latest_since(state.seen) {
                state.present_frame(&frame);
            }
        }
        event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
            std::time::Instant::now() + std::time::Duration::from_millis(16),
        ));
    }
}
