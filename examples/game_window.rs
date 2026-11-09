//! A classic desktop window: winit owns the main thread (macOS requires
//! it), the engine pump runs on another thread through the ordinary
//! `WindowSystem` seam, and the wgpu renderer presents straight to the
//! window surface. Same game as game_stream — a ball falls under rapier —
//! but pixels go to your compositor instead of a browser.
//!
//! Run:
//!   cargo run --example game_window --features physics,wgpu,winit

use std::thread;

use hyg::game::physics::Shape;
use hyg::game::renderable::MeshRenderer;
use hyg::game::camera::Camera;
use hyg::game::winit_window::WinitWindow;
use hyg::game::{Engine, Entity, PhysicsBody, PhysicsWorld, WgpuRenderer};
use hyg::var::Vec3;
use hyg::Var;

fn main() {
    // the event loop + window: created and run on the main thread
    let window = match WinitWindow::new("☿ hyg — game window", (800, 450)) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("no windowing system: {e}");
            std::process::exit(1);
        }
    };
    println!("waiting for the window…");

    // the pump: engine + renderer on another thread
    // the pump handle doubles as the closer (both share the loop's state)
    let pump_window = window.pump_handle();
    let closer = window.pump_handle();
    let pump = thread::spawn(move || {
        // wait for the window (created when the event loop resumes)
        let mut winit_window = None;
        for _ in 0..200 {
            if let Some(w) = closer.window() {
                winit_window = Some(w);
                break;
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }
        let Some(winit_window) = winit_window else {
            eprintln!("window never appeared");
            closer.request_close();
            return;
        };

        let renderer = match pollster::block_on(WgpuRenderer::new_for_window(
            winit_window,
        )) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("no wgpu adapter for the window: {e}");
                closer.request_close();
                return;
            }
        };

        let mut engine = Engine::new("acme", "window-demo");
        engine.frame_time(0.0);
        engine.fixed_dt(1.0 / 60.0);
        engine.set_physics(PhysicsWorld::create("rapier", &Var::Null).unwrap());
        engine.set_camera(Camera::default());
        engine.set_renderer(Box::new(renderer));

        let mut floor = Entity::new("floor");
        floor.add_component(Box::new(
            PhysicsBody::new("floor", Shape::Box(Vec3::new(6.0, 0.5, 6.0))).static_body(),
        ));
        floor.add_component(Box::new(MeshRenderer::rect(
            Vec3::new(6.0, 0.5, 6.0),
            [0.30, 0.34, 0.40, 1.0],
        )));
        engine.add_entity(floor);

        let mut ball = Entity::new("ball");
        ball.transform_mut().position = Vec3::new(0.0, 8.0, 0.0);
        ball.add_component(Box::new(PhysicsBody::new("ball", Shape::Sphere(0.5))));
        ball.add_component(Box::new(MeshRenderer::circle(0.5, [1.0, 0.4, 0.1, 1.0])));
        engine.add_entity(ball);

        println!("pumping: watch the ball fall; close the window to quit");
        // the pump: polls winit events, updates, draws the MeshRenderers,
        // presents to the surface — exits on the window's close button
        engine.start_with(Box::new(pump_window));
        closer.request_close();
    });

    // blocks until the pump asks to close or the user closes the window
    window.run();
    let _ = pump.join();
    println!("done");
}
