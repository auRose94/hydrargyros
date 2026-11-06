//! glTF models: a cube loaded from a checked-in `.gltf` file, streamed to
//! the browser. `Mesh::from_gltf_file` parses the model — node transforms
//! and the material's base color baked into the vertices — and a
//! `MeshRenderer` draws it at the entity's transform. The pump spins the
//! cube so all faces pass the fixed key light: each face shades
//! differently, and the depth buffer keeps the near faces in front.
//!
//! Run:
//!   cargo run --example game_gltf --features wgpu,stream,gltf
//!
//! The page opens automatically (http://127.0.0.1:8401). Hit the page's
//! quit button to end the pump.

use hydrargyros::game::light::{forward_quat, Light};
use hydrargyros::game::render::FrameSink;
use hydrargyros::game::renderable::MeshRenderer;
use hydrargyros::game::{Camera, Engine, Entity, Model, WgpuRenderer};
use hydrargyros::var::{Quat, Vec3};

fn main() {
    let mut engine = Engine::new("acme", "gltf-demo");
    engine.frame_time(0.0);
    // the default camera is a 16:9 view over y ∈ [-1, 9] with a generous
    // depth window (half_extents.z = 64) — the spinning cube's corners
    // sweep to √2·0.8 ≈ 1.13 in z and never approach the near plane

    // the model: next to the crate, a 1-unit cube whose material samples
    // a checked-in checker image — the texture mapping is visible on every
    // face as it spins
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/cube_checker.gltf");
    let model = Model::from_gltf_file(&path).expect("the checked-in cube_checker.gltf loads");

    let mut cube = Entity::new("cube");
    cube.transform_mut().position = Vec3::new(0.0, 4.0, 0.0); // view center
    cube.transform_mut().scale = Vec3::splat(1.6); // the entity scales the model
    cube.add_component(Box::new(MeshRenderer::model(
        std::sync::Arc::new(model),
        [1.0, 1.0, 1.0, 1.0], // white tint: keep the texture's colors
    )));
    engine.add_entity(cube);

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

    // --no-open skips the auto-launch (paste the URL into any browser)
    let open_browser = !std::env::args().any(|a| a == "--no-open");
    let window = hydrargyros::game::StreamingWindow::new("127.0.0.1:8401", sink, open_browser)
        .expect("stream server");
    println!(
        "streaming at {}{}",
        window.url(),
        if open_browser {
            " (opened in your browser)"
        } else {
            " (paste into a browser)"
        }
    );

    // a custom pump: the same three public frame phases start_with runs —
    // update, draw, render — with a spin scripted in between. This is the
    // gold-script pattern (`engine.entities[0].transform.rotation.y += …`)
    // through Rust's entity_mut seam.
    let mut running = true;
    let mut angle = 0.0f32;
    let dt = 1.0 / 60.0;
    while running {
        for event in window.drain() {
            if event.get("type").as_string().as_deref() == Some("quit") {
                running = false;
            }
        }
        if !running {
            break;
        }
        angle += 0.9 * dt as f32;
        if let Some(cube) = engine.entity_mut("cube") {
            cube.transform_mut().rotation = Quat::axis_angle(Vec3::new(0.0, 1.0, 0.0), angle);
        }
        engine.update(dt);
        engine.draw();
        engine.render();
        std::thread::sleep(std::time::Duration::from_millis(16));
    }

    println!("done — streamed until quit");
}
