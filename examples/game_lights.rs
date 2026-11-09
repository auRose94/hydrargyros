//! The lights tour (the `wgpu` feature): all three light kinds —
//! directional sun, warm point, cool spot — over a floor and three cubes,
//! shaded per-fragment with the KHR punctual math. Rendered offscreen (no
//! window, no surface) and saved as a PNG for eyeballing.
//!
//! Run:
//!   cargo run --example game_lights --features wgpu

use hyg::game::camera::{Camera, Projection};
use hyg::game::light::{forward_quat, Light, SceneLight};
use hyg::game::mesh::Mesh;
use hyg::game::model::Model;
use hyg::game::render::RenderBackend;
use hyg::game::transform::Transform;
use hyg::game::WgpuRenderer;
use hyg::var::Vec3;
use std::sync::Arc;

fn main() {
    let mut renderer =
        pollster::block_on(WgpuRenderer::new_offscreen(640, 360)).expect("a wgpu adapter");

    let camera = Camera {
        position: Vec3::new(0.0, 3.0, -5.5),
        target: Vec3::splat(0.0),
        projection: Projection::Perspective {
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 100.0,
            aspect: 640.0 / 360.0,
        },
    };

    // scene: a floor slab and one cube per light, so each kind has a stage
    let floor = Model::single(Arc::new(Mesh::cube(Vec3::new(4.0, 0.1, 4.0))));
    let cube = Model::single(Arc::new(Mesh::cube(Vec3::splat(0.45))));
    let sphere = Model::single(Arc::new(Mesh::sphere(0.45, 32, 16)));

    let at = |x: f32, y: f32, z: f32| Transform {
        position: Vec3::new(x, y, z),
        ..Transform::default()
    };

    // the lights sit AT entity transforms: position = the world origin of
    // the light, rotation's +z = the direction it shines (KHR convention)
    let sun_at = Transform {
        position: Vec3::new(0.0, 4.0, 0.0),
        rotation: forward_quat([-0.35, -1.0, -0.15]),
        scale: Vec3::splat(1.0),
    };
    let lights = vec![
        // soft white sun from above
        SceneLight::from_component(&Light::directional().with_intensity(0.4), &sun_at),
        // warm red point, arm's reach from its cube
        SceneLight::from_component(
            &Light::new()
                .with_color([1.0, 0.25, 0.1])
                .with_intensity(2.5)
                .with_range(6.0),
            &at(-1.9, 1.3, -0.4),
        ),
        // cool blue spot aimed straight down at its sphere
        SceneLight::from_component(
            &Light::spot()
                .with_color([0.3, 0.55, 1.0])
                .with_intensity(3.0)
                .with_cone([0.85, 0.95])
                .with_range(9.0),
            &Transform {
                position: Vec3::new(1.9, 2.6, 0.4),
                rotation: forward_quat([0.0, -1.0, 0.0]),
                scale: Vec3::splat(1.0),
            },
        ),
    ];

    renderer.begin_scene(&camera, &lights);
    renderer.push_model(&at(0.0, -0.1, 0.0), &floor, [0.85, 0.85, 0.85, 1.0]);
    renderer.push_model(&at(0.0, 0.45, 0.6), &cube, [1.0, 1.0, 1.0, 1.0]);
    renderer.push_model(&at(-1.4, 0.45, 0.2), &cube, [1.0, 1.0, 1.0, 1.0]);
    renderer.push_model(&at(1.4, 0.45, 0.2), &sphere, [1.0, 1.0, 1.0, 1.0]);
    renderer.render_frame();

    let path = std::env::temp_dir().join("hyg_lights.png");
    let img: image::RgbaImage = image::ImageBuffer::from_raw(
        640,
        360,
        renderer.screenshot().to_vec(),
    )
    .expect("frame size matches");
    img.save(&path).unwrap();
    println!("lit scene saved: {}", path.display());
    println!("(the three cubes should read sun-lit, red-glowing, blue-spot-lit)");
}
