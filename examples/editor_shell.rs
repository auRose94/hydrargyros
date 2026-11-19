//! The Studio shell, M1 groundwork: a live engine (offscreen renderer +
//! frame sink) pumping on a worker thread, egui panels composited over its
//! frame in a winit window. Hierarchy left, inspector right — click an
//! entity, drag its transform, watch the viewport follow.
//!
//! ```text
//! cargo run --example editor_shell --features editor,physics
//! ```
//!
//! Close the window or press Esc to quit.

use hyg::game::camera::Projection;
use hyg::game::editor::shell;
use hyg::game::physics::PhysicsWorld;
use hyg::game::physics_body::PhysicsBody;
use hyg::game::renderable::MeshRenderer;
use hyg::game::render_wgpu::WgpuRenderer;
use hyg::game::{Camera, Engine, Entity};
use hyg::var::Vec3;

fn main() {
    // the live-reload demo script, written fresh each launch (edit it
    // while the studio runs — the editor reloads on save; break the
    // syntax and the console shows the panic, edit mode resumes)
    std::fs::write(
        "patrol.js",
        "function update(dt) { transform.position.z += dt * 2; }",
    )
    .expect("writes patrol.js");
    println!("demo script: edit ./patrol.js and save — live reload does the rest");

    // the sink the shell presents from — created before the renderer so
    // the attach can hand it over
    let sink = hyg::game::FrameSink::new();
    let engine = build_engine(sink.clone());
    match shell::launch(engine, sink, "Hydrargyros Studio") {
        Ok(()) => println!("studio closed"),
        Err(e) => {
            eprintln!("studio: {e}");
            std::process::exit(1);
        }
    }
}

fn build_engine(sink: hyg::game::FrameSink) -> Engine {
    let mut engine = Engine::new("acme", "studio-demo");
    engine.frame_time(0.0);
    engine.fixed_dt(1.0 / 60.0);
    engine.set_physics(PhysicsWorld::create("rapier", &hyg::Var::Null).unwrap());
    engine.set_camera(Camera {
        position: Vec3::new(0.0, 4.0, -8.0),
        target: Vec3::new(0.0, 1.0, 0.0),
        projection: Projection::Perspective {
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 100.0,
            aspect: 16.0 / 9.0,
        },
    });

    // the offscreen renderer + the sink the shell presents from
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(960, 540))
        .expect("no wgpu adapter (an editor needs a GPU)");
    renderer.attach_sink(sink);
    engine.set_renderer(Box::new(renderer));

    // 3D primitives for the 3D default scene: meshes (cubes/spheres)
    // render through the perspective/VP path — the quad pipeline is the
    // 2D/ortho path, and mixing them under a perspective camera reads
    // as mirrored halves
    let mut floor = Entity::new("floor");
    floor.add_component(Box::new(
        PhysicsBody::new("floor", hyg::game::physics::Shape::Box(Vec3::new(6.0, 0.5, 6.0)))
            .static_body(),
    ));
    floor.add_component(Box::new(MeshRenderer::model(
        std::sync::Arc::new(hyg::game::Model::single(std::sync::Arc::new(
            hyg::game::Mesh::cube(Vec3::new(6.0, 0.5, 6.0)),
        ))),
        [0.30, 0.34, 0.40, 1.0],
    )));
    engine.add_entity(floor);

    let mut ball = Entity::new("ball");
    ball.transform_mut().position = Vec3::new(0.0, 8.0, 0.0);
    ball.add_component(Box::new(PhysicsBody::new(
        "ball",
        hyg::game::physics::Shape::Sphere(0.5),
    )));
    ball.add_component(Box::new(MeshRenderer::model(
        std::sync::Arc::new(hyg::game::Model::single(std::sync::Arc::new(
            hyg::game::Mesh::sphere(0.5, 24, 16),
        ))),
        [1.0, 0.4, 0.1, 1.0],
    )));
    engine.add_entity(ball);

    let mut sun = Entity::new("sun");
    sun.add_component(Box::new(
        hyg::game::Light::directional().with_color([1.0, 0.95, 0.85]).with_intensity(1.5),
    ));
    engine.add_entity(sun);

    // a scripted drifter: the live-reload demo (edit patrol.js while
    // playing — save = reload; the drifter changes behavior instantly)
    let mut drifter = Entity::new("drifter");
    drifter.transform_mut().position = Vec3::new(0.0, 1.5, -2.0);
    let script = hyg::script::Script::new(
        std::fs::read_to_string("patrol.js").expect("patrol.js exists"),
    )
    .expect("the demo script compiles");
    drifter.add_component(Box::new(
        hyg::game::ScriptedComponent::new(script)
            .expect("the demo script arms")
            .with_source_path("patrol.js"),
    ));
    drifter.add_component(Box::new(MeshRenderer::model(
        std::sync::Arc::new(hyg::game::Model::single(std::sync::Arc::new(
            hyg::game::Mesh::sphere(0.25, 16, 12),
        ))),
        [0.4, 0.9, 0.6, 1.0],
    )));
    engine.add_entity(drifter);

    // the scene's main camera — what play renders through (the editor
    // viewport uses its own orbit; this entity is the document's view)
    let mut camera = Entity::new("main camera");
    camera.add_component(Box::new(hyg::game::CameraComponent {
        camera: hyg::game::Camera {
            position: Vec3::new(0.0, 4.0, -8.0),
            target: Vec3::new(0.0, 1.0, 0.0),
            projection: Projection::Perspective {
                fov_y_degrees: 60.0,
                near: 0.1,
                far: 500.0,
                aspect: 16.0 / 9.0,
            },
        },
        main: true,
    }));
    engine.add_entity(camera);

    engine
}
