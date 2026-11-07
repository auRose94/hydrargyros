//! The streaming-page showcase: an orbit-driven physics sandbox combining
//! every rung in one browser page —
//!   - **rapier physics**: a 9-box stack and two balls on a static floor
//!     (gravity flips on space; two boxes carry gold's `mass` knob),
//!   - **v12-scripted components**: a patroller cube whose `update(dt)`
//!     script moves its ENTITY through the engine-side transform sync —
//!     one mover per entity, no `PhysicsBody` on it,
//!   - **the perspective orbit camera**: left-drag orbits, wheel zooms
//!     (gold's fov 60/near 0.1/far 100),
//!   - **lights**: a directional key sun + a warm point lamp by the stack,
//!   - **meshes**: procedural cubes/spheres, tinted, depth-tested, shaded.
//!
//! Run:
//!   cargo run --example game_sandbox --features physics,wgpu,stream,script
//!
//! The page opens automatically (http://127.0.0.1:8402); `--no-open` skips
//! the auto-launch — paste the URL into any browser. Controls: left-drag =
//! orbit, wheel = zoom, space (or " ") = flip gravity, `r` = reset the
//! stack, the page's quit button = end the pump.

use std::sync::Arc;

use hydrargyros::game::camera::OrbitController;
use hydrargyros::game::light::{forward_quat, Light};
use hydrargyros::game::physics::{BodyType, Shape as PhysShape};
use hydrargyros::game::render::FrameSink;
use hydrargyros::game::renderable::MeshRenderer;
use hydrargyros::game::{
    Engine, Entity, Mesh, PhysicsBody, PhysicsWorld, ScriptedComponent, WgpuRenderer,
};
use hydrargyros::script::Script;
use hydrargyros::var::{Quat, Vec3};
use hydrargyros::Var;

fn main() {
    let mut engine = Engine::new("acme", "sandbox-demo");
    engine.frame_time(0.0);
    engine.set_physics(PhysicsWorld::create("rapier", &Var::Null).expect("rapier backend"));
    if let Some(physics) = engine.physics_mut() {
        physics.set_gravity(Vec3::new(0.0, -9.81, 0.0));
    }

    // the spawn table: every DYNAMIC body's initial pose — `r` re-registers
    // each at its spawn pose (gold's replace semantics) and the next
    // frame's physics sync-back restores the entity transforms. The
    // patroller is NOT in the table: it has no PhysicsBody (one mover per
    // entity), so the script keeps driving it through resets.
    let mut spawns: Vec<(String, PhysShape, f32, Vec3)> = Vec::new();

    // the ground: a static box whose top sits at y = 0, flat gray tint
    let mut ground = Entity::new("ground");
    ground.transform_mut().position = Vec3::new(0.0, -0.5, 0.0);
    ground.add_component(Box::new(
        PhysicsBody::new("ground", PhysShape::Box(Vec3::new(8.0, 0.5, 8.0))).static_body(),
    ));
    ground.add_component(Box::new(MeshRenderer::mesh(
        Arc::new(Mesh::cube(Vec3::new(8.0, 0.5, 8.0))),
        [0.32, 0.36, 0.42, 1.0],
    )));
    engine.add_entity(ground);

    // the stack: 4 + 3 + 2 half-unit cubes in a pyramid on the ground top —
    // each its own entity and dynamic body, all sharing ONE cube mesh (the
    // tint colors it). Two carry gold's `mass` knob: an explicit mass
    // weighs the body exactly that (0 keeps the density-derived default).
    let stack: [(f32, f32); 9] = [
        (-0.5, -0.5),
        (0.5, -0.5),
        (-0.5, 0.5),
        (0.5, 0.5), // layer 0, y = 0.5
        (-0.5, 0.5),
        (0.5, 0.5),
        (0.0, -0.5), // layer 1, y = 1.5
        (-0.5, 0.5),
        (0.5, 0.5), // layer 2, y = 2.5
    ];
    let tints: [[f32; 4]; 9] = [
        [0.90, 0.35, 0.28, 1.0], // rust
        [0.95, 0.62, 0.25, 1.0], // amber
        [0.93, 0.85, 0.35, 1.0], // yellow
        [0.45, 0.75, 0.40, 1.0], // green
        [0.30, 0.65, 0.65, 1.0], // teal
        [0.35, 0.55, 0.90, 1.0], // blue
        [0.60, 0.45, 0.85, 1.0], // violet
        [0.90, 0.45, 0.65, 1.0], // pink
        [0.85, 0.55, 0.35, 1.0], // copper
    ];
    let box_mesh = Arc::new(Mesh::cube(Vec3::splat(0.5)));
    for (i, (x, z)) in stack.iter().enumerate() {
        let layer = if i < 4 {
            0
        } else if i < 7 {
            1
        } else {
            2
        };
        let y = layer as f32 + 0.5;
        let name = format!("box-{i}");
        // the two mass knobs: the heaviest box at the base, a feather on
        // top of it — collisions read the difference
        let mass = if i == 0 {
            2.0
        } else if i == 5 {
            0.5
        } else {
            0.0
        };
        let mut block = Entity::new(name.clone());
        block.transform_mut().position = Vec3::new(*x, y, *z);
        let body = PhysicsBody::new(name.clone(), PhysShape::Box(Vec3::splat(0.5)));
        block.add_component(Box::new(if mass > 0.0 { body.mass(mass) } else { body }));
        block.add_component(Box::new(MeshRenderer::mesh(
            Arc::clone(&box_mesh),
            tints[i],
        )));
        engine.add_entity(block);
        spawns.push((
            name,
            PhysShape::Box(Vec3::splat(0.5)),
            mass,
            Vec3::new(*x, y, *z),
        ));
    }

    // two balls dropped from above the stack — they bounce, roll, and knock
    // corners off; shared sphere mesh, per-entity tints
    let balls: [(&str, f32, f32, f32, [f32; 4]); 2] = [
        ("ball-a", 2.8, 3.5, -1.8, [1.0, 0.55, 0.15, 1.0]),
        ("ball-b", -2.6, 4.5, 1.6, [0.25, 0.65, 0.95, 1.0]),
    ];
    let sphere_mesh = Arc::new(Mesh::sphere(0.5, 24, 16));
    for (name, x, y, z, tint) in balls {
        let mut ball = Entity::new(name);
        ball.transform_mut().position = Vec3::new(x, y, z);
        ball.add_component(Box::new(PhysicsBody::new(name, PhysShape::Sphere(0.5))));
        ball.add_component(Box::new(MeshRenderer::mesh(Arc::clone(&sphere_mesh), tint)));
        engine.add_entity(ball);
        spawns.push((
            name.to_string(),
            PhysShape::Sphere(0.5),
            0.0,
            Vec3::new(x, y, z),
        ));
    }

    // the scripted patroller: its OWN entity, NO PhysicsBody — the
    // one-mover rule (physics XOR script; an entity with both syncs in
    // undefined order, documented in both modules). A small bright cube
    // driven by a v12 script — the script IS the interpreter seam, v12
    // underneath (the JSON bridge carries the `transform` global across):
    // top-level `let` state survives across calls (same-eval closures), so
    // `t` accumulates and the patrol circles forever. The engine seeds the
    // script's transform from the entity's spawn pose and syncs the handle
    // back into the entity after every component pass — the draw pass sees
    // the scripted pose with no component-side entity access.
    const PATROL: &str = "let t = 0; \
                          function update(dt) { \
                              t += dt; \
                              transform.position.x = Math.cos(t * 0.8) * 3.0; \
                              transform.position.z = Math.sin(t * 0.8) * 3.0; \
                              transform.position.y = 1.0 + Math.sin(t * 2.0) * 0.3; \
                          }";
    let script = Script::new(PATROL).expect("the patroller script compiles");
    let mut patroller = Entity::new("patroller");
    patroller.transform_mut().position = Vec3::new(3.0, 1.0, 0.0);
    patroller.add_component(Box::new(
        ScriptedComponent::new(script).expect("the patroller bridge arms"),
    ));
    patroller.add_component(Box::new(MeshRenderer::mesh(
        Arc::new(Mesh::cube(Vec3::splat(0.3))),
        [1.0, 0.35, 0.85, 1.0], // bright magenta, easy to track
    )));
    engine.add_entity(patroller);

    // the sun: a directional light aimed like the classic key light (up,
    // right, toward the viewer), plus a warm point lamp hanging near the
    // stack — the scene shades with real depth, not flat
    let mut sun = Entity::new("sun");
    sun.transform_mut().rotation = forward_quat([-0.4, -0.7, -0.5]);
    sun.add_component(Box::new(Light::directional().with_intensity(0.55)));
    engine.add_entity(sun);

    let mut lamp = Entity::new("lamp");
    lamp.transform_mut().position = Vec3::new(3.0, 3.5, 2.0);
    lamp.add_component(Box::new(
        Light::new()
            .with_color([1.0, 0.72, 0.45])
            .with_intensity(1.4)
            .with_range(14.0),
    ));
    engine.add_entity(lamp);

    // the offscreen renderer publishes every frame into the sink
    let sink = FrameSink::new();
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(640, 360))
        .expect("no wgpu adapter on this machine");
    renderer.attach_sink(sink.clone());
    engine.set_renderer(Box::new(renderer));

    // third-person orbit around the stack: gold's fov 60°/near 0.1/far
    // 100, 16:9 like the canvas. The subject is the patroller's orbit —
    // radius 3 plus the 0.3 cube ≈ 3.3 world units — which at distance 12
    // subtends atan(3.3/12) ≈ 15° half-angle, ~31° of the 60° vertical
    // view: comfortably framed (game_gltf's math). The horizontal
    // half-view is atan(tan 30° · 16/9) ≈ 45.8°; the yaw-40° floor's
    // widest corners reach ~11.3 world units along the screen axis at
    // depths ~11.7–13.5, i.e. 40–44° — inside the half-view, with the near
    // edge slipping beneath the eye.
    let mut orbit = OrbitController::new(Vec3::new(0.0, 1.5, 0.0));
    orbit.yaw = 40.0;
    orbit.pitch = 22.0;
    orbit.distance = 12.0;
    engine.set_camera(orbit.camera());

    // --no-open skips the auto-launch (paste the URL into any browser)
    let open_browser = !std::env::args().any(|a| a == "--no-open");
    let window = hydrargyros::game::StreamingWindow::new("127.0.0.1:8402", sink, open_browser)
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
    println!("controls: left-drag orbit · wheel zoom · space = flip gravity · r = reset stack");

    // a custom pump: the same three public frame phases start_with runs —
    // events, update, draw, render — with the orbit pose and the key
    // handling scripted in between (the game_gltf pattern). Fixed dt = 60
    // fps of simulated time, however fast the machine runs.
    let mut running = true;
    let mut gravity_up = false;
    let dt = 1.0 / 60.0;
    while running {
        for event in window.drain() {
            if event.get("type").as_string().as_deref() == Some("quit") {
                running = false;
            }
            // left-drag orbits, wheel zooms (the unified mouse/wheel events)
            orbit.handle(&event);

            // Keys. Name divergence, on purpose: the streaming page sends
            // DOM `e.key`, where spacebar is a literal " "; the winit
            // backend lowercases named keys, where it's "space". Handlers
            // accept BOTH so the same code serves either backend. `pressed`
            // gates on keydown (the page sends keyup too; holding a key
            // auto-repeats keydowns — tap, don't hold).
            if event.get("pressed").as_bool() != Some(true) {
                continue;
            }
            match event.get("key").as_str() {
                // space: flip gravity — (0, −9.81, 0) ↔ (0, 9.81, 0)
                Some("space") | Some(" ") => {
                    gravity_up = !gravity_up;
                    let g = Vec3::new(0.0, if gravity_up { 9.81 } else { -9.81 }, 0.0);
                    if let Some(physics) = engine.physics_mut() {
                        physics.set_gravity(g);
                    }
                    println!(
                        "gravity → (0, {}, 0) — {}",
                        if gravity_up { " 9.81" } else { "-9.81" },
                        if gravity_up {
                            "things rise"
                        } else {
                            "things fall"
                        }
                    );
                }
                // r: re-register every dynamic body at its spawn pose
                // (gold's replace semantics); the next frame's physics
                // sync-back restores the entity transforms
                Some("r") | Some("R") => {
                    if let Some(physics) = engine.physics_mut() {
                        for (name, shape, mass, position) in &spawns {
                            physics.add_body(
                                name,
                                shape,
                                BodyType::Dynamic,
                                *position,
                                Quat::new(0.0, 0.0, 0.0, 1.0),
                                1.0,
                                *mass,
                            );
                        }
                    }
                    println!("reset → {} bodies back at their spawn poses", spawns.len());
                }
                _ => {}
            }
        }
        if !running {
            break;
        }
        engine.set_camera(orbit.camera());
        engine.update(dt);
        engine.draw();
        engine.render();
        std::thread::sleep(std::time::Duration::from_millis(16));
    }

    println!("done — streamed until quit");
}
