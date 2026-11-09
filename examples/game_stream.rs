//! The streaming window: the headless render buffer becomes a browser
//! page. A ball falls under rapier physics, the wgpu renderer draws it
//! offscreen, and a tiny web app streams the frames while browser keys and
//! mouse events flow back into the engine through the normal
//! `WindowSystem::poll` seam — the engine can't tell it apart from SDL.
//!
//! Run:
//!   cargo run --example game_stream --features physics,wgpu,stream
//!
//! The page opens automatically (http://127.0.0.1:8400). Watch the ball
//! fall; press keys, click around — events print in the console; hit the
//! page's quit button to end the pump.

use hyg::game::light::{forward_quat, Light};
use hyg::game::physics::Shape as PhysShape;
use hyg::game::render::FrameSink;
use hyg::game::renderable::MeshRenderer;
use hyg::game::{Camera, Engine, Entity, Mesh, PhysicsBody, PhysicsWorld, WgpuRenderer};
use hyg::var::Vec3;
use hyg::Var;

fn main() {
    let mut engine = Engine::new("acme", "stream-demo");
    engine.frame_time(0.0);
    engine.fixed_dt(1.0 / 60.0); // 60 fps of simulated time, however fast we render
    engine.set_physics(PhysicsWorld::create("rapier", &Var::Null).unwrap());
    // the default camera is a 16:9 view over y ∈ [-1, 9] — square pixels

    let mut floor = Entity::new("floor");
    floor.add_component(Box::new(
        PhysicsBody::new("floor", PhysShape::Box(Vec3::new(6.0, 0.5, 6.0))).static_body(),
    ));
    floor.add_component(Box::new(MeshRenderer::rect(
        Vec3::new(6.0, 0.5, 6.0),
        [0.30, 0.34, 0.40, 1.0],
    )));
    engine.add_entity(floor);

    let mut ball = Entity::new("ball");
    ball.transform_mut().position = Vec3::new(0.0, 8.0, 0.0);
    ball.add_component(Box::new(PhysicsBody::new("ball", PhysShape::Sphere(0.5))));
    // the ball is a real triangle mesh now: a shaded sphere instead of a
    // flat disc — the key light reads its normals, so it looks round
    ball.add_component(Box::new(MeshRenderer::mesh(
        std::sync::Arc::new(Mesh::sphere(0.5, 24, 16)),
        [1.0, 0.4, 0.1, 1.0],
    )));
    engine.add_entity(ball);

    // the sun: a directional light component aimed like the classic key
    // light (up, right, toward the viewer) — lights are scene data now
    let mut sun = Entity::new("sun");
    sun.transform_mut().rotation = forward_quat([-0.4, -0.7, -0.5]);
    sun.add_component(Box::new(Light::directional().with_intensity(0.55)));
    engine.add_entity(sun);

    // the offscreen renderer publishes every frame into the sink
    let sink = FrameSink::new();
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.attach_sink(sink.clone());
    engine.set_renderer(Box::new(renderer));
    engine.set_camera(Camera::default());

    #[cfg(not(feature = "tray"))]
    println!("(build with --features tray for a system-tray quit button)");

    // the streaming window: browser events come back as Var.
    // --no-open skips the auto-launch (paste the URL into any browser —
    // including an embedded one like ZCode's).
    let open_browser = !std::env::args().any(|a| a == "--no-open");
    let window = hyg::game::StreamingWindow::new("127.0.0.1:8400", sink, open_browser)
        .expect("stream server");
    let window = std::sync::Arc::new(window);

    // tray icon: Quit reaches the engine even with no visible window
    #[cfg(feature = "tray")]
    {
        let window = window.clone();
        match hyg::game::spawn_tray_quit("hyg (streaming)", move |quit| {
            window.inject(quit);
        }) {
            Ok(()) => println!("tray icon up (Quit closes the game)"),
            Err(e) => println!("tray unavailable: {e}"),
        }
    }

    println!(
        "streaming at {}{}",
        window.url(),
        if open_browser {
            " (opened in your browser)"
        } else {
            " (paste into a browser)"
        }
    );

    // the pump: polls the page, updates, draws the MeshRenderers, renders —
    // exits when the page's Quit button arrives through the event seam
    engine.start_with(Box::new(window));

    println!("done — streamed until quit");
}
