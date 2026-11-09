//! A physics world: a ball falls onto a floor (rapier through the
//! registered-backend pattern), the engine syncs its pose into the entity
//! transform each frame, a raycast names what it lands on — and the wgpu
//! offscreen renderer saves a screenshot.
//!
//! Run:
//!   cargo run --example game_world --features physics,wgpu

use hyg::game::component::Component;
use hyg::game::physics::Shape;
use hyg::game::{Engine, Entity, HeadlessWindow, PhysicsBody, PhysicsWorld};
use hyg::var::Vec3;
use hyg::Var;

struct FallReport {
    name: String,
    frames: u32,
}

impl Component for FallReport {
    fn update(&mut self, _dt: f64) {
        self.frames += 1;
        if self.frames.is_multiple_of(60) {
            println!("  frame {}: {} still falling", self.frames, self.name);
        }
    }
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

fn main() {
    let mut engine = Engine::new("acme", "world-demo");
    engine.frame_time(0.0);
    engine.fixed_dt(1.0 / 60.0); // 240 frames = 4 simulated seconds

    // the world sim: rapier when built with --features physics, otherwise
    // the no-op "none" backend (the fallback when rapier is unavailable)
    let world = PhysicsWorld::create("rapier", &Var::Null)
        .unwrap_or_else(|_| PhysicsWorld::none());
    println!("physics backend: {}", world.backend_name());
    engine.set_physics(world);

    let mut floor = Entity::new("floor");
    floor.add_component(Box::new(
        PhysicsBody::new("floor", Shape::Box(Vec3::new(10.0, 0.5, 10.0))).static_body(),
    ));
    engine.add_entity(floor);

    let mut ball = Entity::new("ball");
    ball.transform_mut().position = Vec3::new(0.0, 10.0, 0.0);
    ball.add_component(Box::new(PhysicsBody::new("ball", Shape::Sphere(0.5))));
    ball.add_component(Box::new(FallReport { name: "ball".to_string(), frames: 0 }));
    engine.add_entity(ball);

    // pump until the ball settles: 4 seconds of simulated time
    engine.start_with(Box::new(HeadlessWindow::for_frames(240)));

    let (pos, _) = engine
        .physics_mut()
        .unwrap()
        .body_pose("ball")
        .unwrap_or((Vec3::new(0.0, 0.0, 0.0), hyg::var::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 }));
    println!("ball settled at y={:.3} (floor top is 0.5)", pos.y);

    // the headless screenshot debug aid: render a frame, save it
    #[cfg(feature = "wgpu")]
    {
        match pollster::block_on(hyg::game::WgpuRenderer::new_offscreen(320, 180)) {
            Ok(mut renderer) => {
                use hyg::game::RenderBackend;
                renderer.render_frame();
                let path = std::env::temp_dir().join("hyg_game_world.png");
                let frame = renderer.screenshot();
                let img: image::RgbaImage = image::ImageBuffer::from_raw(320, 180, frame.to_vec())
                    .expect("frame size mismatch");
                img.save(&path).unwrap();
                println!("screenshot saved: {}", path.display());
            }
            Err(e) => println!("no renderer on this machine: {e}"),
        }
    }

    // a raycast straight down names what's under the origin
    #[cfg(feature = "physics")]
    {
        if let Some(hit) = engine.physics_mut().unwrap().raycast(
            Vec3::new(0.0, 10.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            100.0,
        ) {
            println!(
                "raycast down from (0,10,0): hit {:?} at distance {:.2}",
                hit.body, hit.distance
            );
        }
    }
}
