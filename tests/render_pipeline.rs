//! Rendering-pipeline tests: the camera maps world → pixels, circles clip
//! to discs, and entities render themselves through the full
//! transform → camera → pixel pipeline.

#![cfg(feature = "wgpu")]

use hyg::game::camera::Camera;
use hyg::game::renderable::MeshRenderer;
use hyg::game::HeadlessWindow;
use hyg::game::{
    Engine, Entity, PhysicsWorld, RenderBackend, StreamingWindow, WgpuRenderer,
};
use hyg::o;
use hyg::var::Vec3;

fn pixel(frame: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]
}

fn srgb8(linear: f64) -> u8 {
    let v = if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (v * 255.0).round() as u8
}

fn assert_close(got: [u8; 4], want: [u8; 4], what: &str) {
    for (g, w) in got.iter().zip(want) {
        assert!(
            (*g as i32 - w as i32).abs() <= 3,
            "{what}: {got:?} vs {want:?}"
        );
    }
}

/// The camera maps the world window onto the canvas: view center → canvas
/// center, view edges → edges.
#[test]
fn camera_maps_world_to_clip() {
    let camera = Camera::ortho(Vec3::new(0.0, 4.0, 0.0), Vec3::new(8.0, 4.5, 1.0));
    let center = camera.world_to_clip(Vec3::new(0.0, 4.0, 0.0));
    assert_eq!(center, [0.0, 0.0]);

    // view corners: +y up on the screen
    let top_left = camera.world_to_clip(Vec3::new(-8.0, 8.5, 0.0));
    assert_eq!(top_left, [-1.0, 1.0]);
    let bottom_right = camera.world_to_clip(Vec3::new(8.0, -0.5, 0.0));
    assert_eq!(bottom_right, [1.0, -1.0]);
}

#[tokio::test]
async fn world_space_quad_lands_where_the_camera_says() {
    // camera centered at (0, 4), half-extents (8, 4.5): a quad at the view
    // center must land at the canvas center
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
    renderer.begin_scene(
        &Camera::ortho(Vec3::new(0.0, 4.0, 0.0), Vec3::new(8.0, 4.5, 1.0)),
        &[], // quads are unlit: an empty light list is fine here
    );
    renderer.push_quad(hyg::game::Quad::rect(
        [0.0, 4.0],
        [0.5, 0.5],
        [1.0, 0.0, 0.0, 1.0],
    ));
    renderer.render_frame();

    let frame = renderer.screenshot();
    // canvas center: inside the quad → red
    assert_close(
        pixel(frame, 480, 240, 135),
        [srgb8(1.0), 0, 0, 255],
        "center",
    );
    // a corner: clear
    assert_close(pixel(frame, 480, 10, 10), [0, 0, 0, 255], "corner");
}

#[tokio::test]
async fn circle_shape_clips_to_a_disc() {
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(240, 240))
        .expect("no wgpu adapter on this machine");
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
    renderer.set_scene(&[hyg::game::Quad::circle(
        [0.0, 0.0],
        [0.5, 0.5],
        [0.0, 1.0, 0.0, 1.0],
    )]);
    renderer.render_frame();

    let frame = renderer.screenshot();
    // the disc's center: green
    assert_close(
        pixel(frame, 240, 120, 120),
        [0, srgb8(1.0), 0, 255],
        "disc center",
    );
    // the bbox corner (NDC 0.7, 0.7 → length 0.99? no: (0.7,0.7) local ≈ 1.4
    // in unit-disc space — outside the disc): clear
    let corner = pixel(frame, 240, 204, 36);
    assert_eq!(corner, [0, 0, 0, 255]);
}

/// The full pipeline: an entity's MeshRenderer draws through the camera at
/// its entity's world transform — no manual mapping anywhere.
#[tokio::test]
async fn entity_meshrenderer_renders_through_camera() {
    let sink = hyg::game::FrameSink::new();
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.attach_sink(sink.clone());

    let mut engine = Engine::new("acme", "pipeline-test");
    engine.set_physics(PhysicsWorld::none());
    engine.set_renderer(Box::new(renderer));

    let mut quad = Entity::new("marked");
    quad.transform_mut().position = Vec3::new(0.0, 4.0, 0.0); // view center
    quad.add_component(Box::new(MeshRenderer::rect(
        Vec3::new(1.0, 1.0, 1.0),
        [1.0, 0.0, 0.0, 1.0],
    )));
    engine.add_entity(quad);

    // one pumped frame: update → draw → render_frame → publish
    engine.add_entity(key_sun());
    engine.start_with(Box::new(HeadlessWindow::for_frames(1)));

    let frame = sink.latest().expect("no frame published");
    let center = pixel(&frame.rgba, frame.width, frame.width / 2, frame.height / 2);
    assert_close(center, [srgb8(1.0), 0, 0, 255], "center");

    // the quad is 1 world half-unit tall on a 5-half-unit view → it spans
    // 1/5 of the canvas height: an edge just inside the boundary must be
    // red too (a scale bug shrinks or grows the quad from the center)
    let edge_y = 135 - (1.0 / 5.0 * 135.0) as u32 + 3;
    let edge = pixel(&frame.rgba, frame.width, frame.width / 2, edge_y);
    assert_close(edge, [srgb8(1.0), 0, 0, 255], "quad edge");
}

/// A child entity renders at its COMPOSED world transform — gold's
/// transform hierarchy reaches the pixels.
#[tokio::test]
async fn child_entities_render_at_world_transforms() {
    let sink = hyg::game::FrameSink::new();
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.attach_sink(sink.clone());

    let mut engine = Engine::new("acme", "hierarchy-test");
    engine.set_physics(PhysicsWorld::none());
    engine.set_renderer(Box::new(renderer));
    // default camera: center (0, 4), half (8.89, 5)

    let mut parent = Entity::new("parent");
    parent.transform_mut().position = Vec3::new(0.0, 4.0, 0.0);
    let mut child = Entity::new("child");
    child.transform_mut().position = Vec3::new(4.0, 0.0, 0.0); // right of parent
    child.add_component(Box::new(MeshRenderer::circle(1.0, [0.0, 1.0, 0.0, 1.0])));
    parent.add_child(child);
    engine.add_entity(parent);

    engine.add_entity(key_sun());
    engine.start_with(Box::new(HeadlessWindow::for_frames(1)));

    let frame = sink.latest().expect("no frame");
    // child's world (4, 4) → clip (4/8.89, 0) → pixel (240 + 4/8.89*240, 135)
    // ≈ (348, 135): green disc center
    let px = (240.0 + 4.0 / 8.89 * 240.0) as u32;
    let center = pixel(&frame.rgba, frame.width, px, 135);
    assert_close(center, [0, srgb8(1.0), 0, 255], "child disc");
}

/// StreamingWindow events and renderables coexist: the window injects a
/// quit while the scene renders (the AI-debug loop, end to end).
#[tokio::test]
async fn stream_window_with_renderables() {
    use std::sync::Arc;

    let sink = hyg::game::FrameSink::new();
    let mut renderer =
        pollster::block_on(WgpuRenderer::new_offscreen(320, 180)).expect("no wgpu adapter");
    renderer.attach_sink(sink.clone());

    let mut engine = Engine::new("acme", "stream-pipeline");
    engine.set_physics(PhysicsWorld::none());
    engine.set_renderer(Box::new(renderer));
    engine.add_entity(Entity::new("witness"));

    let window = Arc::new(StreamingWindow::new("127.0.0.1:0", sink, false).unwrap());
    // quit arrives through the window seam before the first frame ends
    window.inject(o! { "type": "quit" }.into());
    engine.start_with(Box::new(window));

    // one pumped frame ran, then the quit stopped the pump
    assert!(!engine.running());
}

// ---- meshes (the triangle pipeline) ----

use hyg::game::light::{forward_quat, Light, LightKind, SceneLight};
use hyg::game::mesh::Mesh;
use hyg::game::model::Model;
use hyg::game::Transform;
use hyg::var::{Quat, Vec3 as V3};

/// The mesh shader's fixed key light, mirrored from `WGSL_MESH`:
/// light = normalize(0.4, 0.7, 0.5), shade = 0.45 + 0.55·max(n·light, 0).
fn mesh_shade(normal: [f32; 3]) -> f64 {
    let len = (0.4f64 * 0.4 + 0.7 * 0.7 + 0.5 * 0.5).sqrt();
    let d = (0.4 * normal[0] as f64 + 0.7 * normal[1] as f64 + 0.5 * normal[2] as f64) / len;
    0.45 + 0.55 * d.max(0.0)
}

/// The classic key light as scene data: a directional light whose forward
/// points where the old hardcoded shader light shone from — so
/// `mesh_shade` stays the expected-value formula.
fn key_lights() -> Vec<SceneLight> {
    let l = (0.4f32 * 0.4 + 0.7 * 0.7 + 0.5 * 0.5).sqrt();
    vec![SceneLight {
        position: V3::splat(0.0), // irrelevant for a directional light
        direction: V3::new(-0.4 / l, -0.7 / l, -0.5 / l),
        kind: LightKind::Directional,
        color: [1.0, 1.0, 1.0],
        intensity: 0.55,
        cone: [1.0, 1.0],
        range: 0.0,
    }]
}

/// A sun entity: a directional light component whose entity forward aims
/// it like the classic key light (the engine gathers light components at
/// their entity's world transform).
fn key_sun() -> Entity {
    let mut sun = Entity::new("sun");
    sun.transform_mut().rotation = forward_quat([-0.4, -0.7, -0.5]);
    sun.add_component(Box::new(Light::directional().with_intensity(0.55)));
    sun
}

/// A cube with per-face vertex colors. `Mesh::cube` emits faces in table
/// order (+x, −x, +y, −y, +z, −z), 4 vertices each.
fn colored_cube(face_colors: [[f32; 4]; 6]) -> std::sync::Arc<Model> {
    let cube = Mesh::cube(V3::splat(0.8));
    let mut verts = cube.vertices().to_vec();
    for (i, v) in verts.iter_mut().enumerate() {
        v.color = face_colors[i / 4];
    }
    let mesh =
        std::sync::Arc::new(Mesh::new(verts, cube.indices().to_vec()).expect("cube rebuild"));
    std::sync::Arc::new(Model::single(mesh))
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

/// A mesh cube renders at its entity transform: the +z face faces the 2D
/// ortho view and lambert-shades; everything outside stays clear.
#[tokio::test]
async fn mesh_cube_shades_at_its_entity_transform() {
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
    renderer.begin_scene(
        &Camera::ortho(V3::new(0.0, 4.0, 0.0), V3::new(8.89, 5.0, 2.0)), // z is the depth window
        &key_lights(),
    );
    renderer.push_model(
        &Transform {
            position: V3::new(0.0, 4.0, 0.0),
            ..Default::default()
        },
        &colored_cube([WHITE, WHITE, WHITE, WHITE, RED, WHITE]),
        WHITE,
    );
    renderer.render_frame();

    let frame = renderer.screenshot();
    // front face (+z): red, shaded by the key light
    let want = [srgb8(mesh_shade([0.0, 0.0, 1.0])), 0, 0, 255];
    assert_close(pixel(frame, 480, 240, 135), want, "front face");
    // the face spans ±0.8 world → ±38 columns; just past its edge: clear
    let outside = (240.0 + 0.95 / 8.89 * 240.0) as u32;
    assert_close(
        pixel(frame, 480, outside, 135),
        [0, 0, 0, 255],
        "past the edge",
    );
}

/// The model matrix follows the entity: 90° about Y brings the −x face to
/// the front, and a z×2 scale widens it on screen (scale applies before
/// rotation).
#[tokio::test]
async fn mesh_transforms_follow_the_entity() {
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
    renderer.begin_scene(
        &Camera::ortho(V3::new(0.0, 4.0, 0.0), V3::new(8.89, 5.0, 2.0)),
        &key_lights(),
    );
    renderer.push_model(
        &Transform {
            position: V3::new(0.0, 4.0, 0.0),
            rotation: Quat::axis_angle(V3::new(0.0, 1.0, 0.0), std::f32::consts::FRAC_PI_2),
            scale: V3::new(1.0, 1.0, 2.0),
        },
        &colored_cube([WHITE, GREEN, WHITE, WHITE, WHITE, WHITE]),
        WHITE,
    );
    renderer.render_frame();

    let frame = renderer.screenshot();
    let green_shaded = [0, srgb8(mesh_shade([0.0, 0.0, 1.0])), 0, 255];
    // the rotated −x face now fronts the view, dead center
    assert_close(pixel(frame, 480, 240, 135), green_shaded, "rotated face");
    // scale z=2 → the face spans ±1.6 world on screen: 1.4 is still on it
    let inside = (240.0 + 1.4 / 8.89 * 240.0) as u32;
    assert_close(pixel(frame, 480, inside, 135), green_shaded, "scaled edge");
    // 1.9 is past it (the neighboring face is edge-on): clear
    let outside = (240.0 + 1.9 / 8.89 * 240.0) as u32;
    assert_close(
        pixel(frame, 480, outside, 135),
        [0, 0, 0, 255],
        "past the scale",
    );
}

/// Meshes depth-test: the nearer cube wins the shared pixel even though
/// the farther one is queued (and drawn) first.
#[tokio::test]
async fn meshes_depth_test_against_each_other() {
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
    renderer.begin_scene(
        &Camera::ortho(V3::new(0.0, 4.0, 0.0), V3::new(8.89, 5.0, 2.0)),
        &key_lights(),
    );
    // far cube first, tinted green; near cube after, tinted red
    renderer.push_model(
        &Transform {
            position: V3::new(0.0, 4.0, -0.5),
            ..Default::default()
        },
        &colored_cube([WHITE; 6]),
        GREEN,
    );
    renderer.push_model(
        &Transform {
            position: V3::new(0.0, 4.0, 0.5),
            ..Default::default()
        },
        &colored_cube([WHITE; 6]),
        RED,
    );
    renderer.render_frame();

    let frame = renderer.screenshot();
    // painter's order would put green on top; the depth buffer keeps red
    let want = [srgb8(mesh_shade([0.0, 0.0, 1.0])), 0, 0, 255];
    assert_close(pixel(frame, 480, 240, 135), want, "near cube wins");
}

/// The depth window clips: a cube whose front face crosses the camera's
/// near plane (`position.z + half_extents.z`) loses that face — and with
/// back-face culling the view falls straight through to the clear color —
/// while a cube inside the window renders normally beside it.
#[tokio::test]
async fn mesh_geometry_beyond_the_depth_window_clips() {
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
    renderer.begin_scene(
        &Camera::ortho(V3::new(0.0, 4.0, 0.0), V3::new(8.89, 5.0, 1.0)), // tight depth window on purpose
        &key_lights(),
    );
    let cube = colored_cube([WHITE; 6]);
    // this cube spans z ∈ [-0.3, 1.3]: its front face is past the near
    // plane (+1.0), so it clips and the center shows clear color (a
    // GL-style [-1, 1] mapping would instead render it bright red here)
    renderer.push_model(
        &Transform {
            position: V3::new(0.0, 4.0, 0.5),
            ..Default::default()
        },
        &cube,
        RED,
    );
    // this cube spans z ∈ [-0.8, 0.8], fully inside the window → renders
    renderer.push_model(
        &Transform {
            position: V3::new(5.0, 4.0, 0.0),
            ..Default::default()
        },
        &cube,
        RED,
    );
    renderer.render_frame();

    let frame = renderer.screenshot();
    assert_close(
        pixel(frame, 480, 240, 135),
        [0, 0, 0, 255],
        "near-clipped face",
    );
    let inside = (240.0 + 5.0 / 8.89 * 240.0) as u32;
    let want = [srgb8(mesh_shade([0.0, 0.0, 1.0])), 0, 0, 255];
    assert_close(pixel(frame, 480, inside, 135), want, "inside the window");
}

// ---- camera 3D (perspective/look-at per gold's bx conventions) ----

use hyg::game::camera::Projection;

/// Project a world point through the camera → NDC (x right, y up).
fn project(camera: &Camera, p: V3) -> [f32; 2] {
    let m = camera.view_projection();
    let v = [p.x, p.y, p.z, 1.0];
    let mut clip = [0.0f32; 4];
    for (r, cell) in clip.iter_mut().enumerate() {
        *cell = (0..4).map(|c| m[c][r] * v[c]).sum();
    }
    [clip[0] / clip[3], clip[1] / clip[3]]
}

/// NDC → pixel coordinates on a `w`×`h` canvas (+x right, +y up).
fn ndc_px(w: u32, h: u32, ndc: [f32; 2]) -> (f64, f64) {
    (
        (ndc[0] + 1.0) as f64 / 2.0 * w as f64,
        (1.0 - ndc[1]) as f64 / 2.0 * h as f64,
    )
}

/// The 3D test rig: the eye 4 units in front of the origin (on −z) looking
/// down +z at a cube sitting there — gold's fov 60 / near 0.1 / far 100 at
/// the canvas's 16:9. Sees the cube's −z face; the key light can't reach
/// it, so it shades to the 0.45 ambient floor.
fn perspective_camera() -> Camera {
    Camera {
        position: V3::new(0.0, 0.0, -4.0),
        target: V3::splat(0.0),
        projection: Projection::Perspective {
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 100.0,
            aspect: 480.0 / 270.0,
        },
    }
}

/// The model renders through the PERSPECTIVE camera: sample points come
/// from projected world points — just inside the front face's projected
/// edges the shaded face shows, just outside (and past the corner), clear.
#[tokio::test]
async fn perspective_scene_renders_the_model() {
    let camera = perspective_camera();
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
    renderer.begin_scene(&camera, &key_lights());
    renderer.push_model(
        &Transform {
            position: V3::splat(0.0),
            ..Default::default()
        },
        &colored_cube([WHITE; 6]),
        RED,
    );
    renderer.render_frame();

    // front face at z = −0.8 (view depth 3.2): the ±0.8 edges project to
    // ndc ±scale·0.8/3.2 ≈ ±0.22 → columns ≈ 182..298, rows ≈ 77..193
    // (the right-handed view mirrors x for this +z-looking rig, so sample
    // by min/max of the projected edges, never by world side)
    let want = [srgb8(mesh_shade([0.0, 0.0, -1.0])), 0, 0, 255];
    let frame = renderer.screenshot();
    let (ex0, row) = ndc_px(480, 270, project(&camera, V3::new(0.8, 0.0, -0.8)));
    let (ex1, _) = ndc_px(480, 270, project(&camera, V3::new(-0.8, 0.0, -0.8)));
    let (lo, hi) = (ex0.min(ex1), ex0.max(ex1));
    let (_, top) = ndc_px(480, 270, project(&camera, V3::new(0.0, 0.8, -0.8)));
    // just inside each projected edge: the face (edges, not centers — a
    // scale bug grows/shrinks the projection and the samples miss)
    assert_close(
        pixel(frame, 480, hi as u32 - 4, row as u32),
        want,
        "inside right edge",
    );
    assert_close(
        pixel(frame, 480, lo as u32 + 4, row as u32),
        want,
        "inside left edge",
    );
    assert_close(
        pixel(frame, 480, 240, top as u32 + 4),
        want,
        "inside top edge",
    );
    // past the top corner of the projected face: clear
    assert_close(
        pixel(frame, 480, hi as u32 + 6, top as u32 - 6),
        [0, 0, 0, 255],
        "past the corner",
    );
}

/// Foreshortening: two same-size cubes at different depths — the nearer
/// face spans MORE pixels (its projected edges land farther apart), and
/// each face shows its tint at the projected edge samples.
#[tokio::test]
async fn perspective_foreshortens_same_size_cubes() {
    let camera = perspective_camera();
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
    renderer.begin_scene(&camera, &key_lights());
    // near cube left (front face at view depth 3.2), far cube right (7.2)
    renderer.push_model(
        &Transform {
            position: V3::new(-0.9, 0.0, 0.0),
            ..Default::default()
        },
        &colored_cube([WHITE; 6]),
        RED,
    );
    renderer.push_model(
        &Transform {
            position: V3::new(1.8, 0.0, 4.0),
            ..Default::default()
        },
        &colored_cube([WHITE; 6]),
        GREEN,
    );
    renderer.render_frame();

    let frame = renderer.screenshot();
    let red = [srgb8(mesh_shade([0.0, 0.0, -1.0])), 0, 0, 255];
    let green = [0, srgb8(mesh_shade([0.0, 0.0, -1.0])), 0, 255];
    let (nx0, row) = ndc_px(480, 270, project(&camera, V3::new(-1.7, 0.0, -0.8)));
    let (nx1, _) = ndc_px(480, 270, project(&camera, V3::new(-0.1, 0.0, -0.8)));
    let (nlo, nhi) = (nx0.min(nx1), nx0.max(nx1));
    let (fx0, _) = ndc_px(480, 270, project(&camera, V3::new(1.0, 0.0, 3.2)));
    let (fx1, _) = ndc_px(480, 270, project(&camera, V3::new(2.6, 0.0, 3.2)));
    let (flo, fhi) = (fx0.min(fx1), fx0.max(fx1));
    // the spans, from the projected corners: nearer covers more pixels
    let (near_span, far_span) = (nhi - nlo, fhi - flo);
    assert!(
        near_span > far_span,
        "near {near_span}px vs far {far_span}px"
    );
    // edge samples: inside each face its tint, outside it clear
    assert_close(
        pixel(frame, 480, nhi as u32 - 4, row as u32),
        red,
        "near inside right",
    );
    assert_close(
        pixel(frame, 480, nhi as u32 + 6, row as u32),
        [0, 0, 0, 255],
        "near outside right",
    );
    assert_close(
        pixel(frame, 480, nlo as u32 + 4, row as u32),
        red,
        "near inside left",
    );
    assert_close(
        pixel(frame, 480, nlo as u32 - 6, row as u32),
        [0, 0, 0, 255],
        "near outside left",
    );
    assert_close(
        pixel(frame, 480, fhi as u32 - 4, row as u32),
        green,
        "far inside right",
    );
    // well past the far cube (clear of its visible side faces too)
    assert_close(
        pixel(frame, 480, fhi as u32 + 10, row as u32),
        [0, 0, 0, 255],
        "far outside right",
    );
    assert_close(
        pixel(frame, 480, flo as u32 - 6, row as u32),
        [0, 0, 0, 255],
        "between cubes",
    );
}

/// Beyond the far plane clips: a cube fully past `far` draws nothing (its
/// would-be pixels stay clear), while a control cube inside the window
/// renders beside it.
#[tokio::test]
async fn perspective_beyond_the_far_plane_clips() {
    let camera = Camera {
        position: V3::new(0.0, 0.0, -4.0),
        target: V3::splat(0.0),
        projection: Projection::Perspective {
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 20.0, // tight far plane on purpose
            aspect: 480.0 / 270.0,
        },
    };
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
    renderer.begin_scene(&camera, &key_lights());
    // front face at view depth 21.5 (> far 20): every vertex is past the
    // far plane, so the whole cube clips
    renderer.push_model(
        &Transform {
            position: V3::new(0.0, 0.0, 17.0),
            ..Default::default()
        },
        &colored_cube([WHITE; 6]),
        RED,
    );
    // control: inside the window, off to the right → renders
    renderer.push_model(
        &Transform {
            position: V3::new(2.0, 0.0, 0.0),
            ..Default::default()
        },
        &colored_cube([WHITE; 6]),
        GREEN,
    );
    renderer.render_frame();

    let frame = renderer.screenshot();
    // the far cube would project to a ~17px patch around the center
    // (corners ±0.8 at depth 21.5 → ndc ±0.036): those pixels stay clear
    let (gx0, row) = ndc_px(480, 270, project(&camera, V3::new(0.8, 0.0, 16.2)));
    let (gx1, _) = ndc_px(480, 270, project(&camera, V3::new(-0.8, 0.0, 16.2)));
    let (glo, ghi) = (gx0.min(gx1), gx0.max(gx1));
    for x in [glo as u32 + 4, 240, ghi as u32 - 4] {
        assert_close(
            pixel(frame, 480, x, row as u32),
            [0, 0, 0, 255],
            "far-clipped",
        );
    }
    // the control cube's front face spans ndc [scale·1.2/3.2, scale·2.8/3.2]
    // (mirrored for this rig): just inside its outer edge, green; just
    // outside it, clear (its −x side face fills the region beyond, so only
    // the outer side is sampled)
    let (cx0, _) = ndc_px(480, 270, project(&camera, V3::new(1.2, 0.0, -0.8)));
    let (cx1, _) = ndc_px(480, 270, project(&camera, V3::new(2.8, 0.0, -0.8)));
    let clo = cx0.min(cx1);
    let green = [0, srgb8(mesh_shade([0.0, 0.0, -1.0])), 0, 255];
    assert_close(
        pixel(frame, 480, clo as u32 + 4, row as u32),
        green,
        "control inside",
    );
    assert_close(
        pixel(frame, 480, clo as u32 - 6, row as u32),
        [0, 0, 0, 255],
        "control outside",
    );
}

/// The checked-in glTF model renders through the full engine pump: baked
/// material color × key-light shade on the front face.
#[cfg(feature = "gltf")] // loads a glTF file (wgpu-only builds stay compilable)
#[tokio::test]
async fn gltf_model_renders_through_the_engine() {
    let sink = hyg::game::FrameSink::new();
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.attach_sink(sink.clone());
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);

    let mut engine = Engine::new("acme", "gltf-pipeline-test");
    engine.set_physics(PhysicsWorld::none());
    engine.set_renderer(Box::new(renderer));

    let model = Model::from_gltf_bytes(include_bytes!("../assets/cube.gltf"))
        .expect("the checked-in cube.gltf loads");
    let mut cube = Entity::new("cube");
    cube.transform_mut().position = V3::new(0.0, 4.0, 0.0);
    cube.add_component(Box::new(MeshRenderer::model(
        std::sync::Arc::new(model),
        [1.0, 1.0, 1.0, 1.0], // white tint: keep the baked material color
    )));
    engine.add_entity(cube);

    engine.add_entity(key_sun());
    engine.start_with(Box::new(HeadlessWindow::for_frames(1)));

    let frame = sink.latest().expect("no frame published");
    let s = mesh_shade([0.0, 0.0, 1.0]);
    let want = [srgb8(0.9 * s), srgb8(0.3 * s), srgb8(0.1 * s), 255];
    assert_close(
        pixel(&frame.rgba, frame.width, frame.width / 2, frame.height / 2),
        want,
        "gltf cube",
    );
}

// ---- textures (sampled by the mesh pipeline) ----

use hyg::game::texture::Texture;

/// sRGB-decode a stored byte the way sampling an Rgba8UnormSrgb texture
/// does (the mirror of `srgb8`).
fn srgb_decode(v: u8) -> f64 {
    let c = v as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// A sprite (textured quad) samples its texture at the vertex UVs: the
/// image's top-left lands on the quad's top-left corner (upright on the
/// +y-up screen), and each texel is tinted by the key-light shade.
#[tokio::test]
async fn sprite_samples_the_texture_upright() {
    let sink = hyg::game::FrameSink::new();
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.attach_sink(sink.clone());
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);

    let mut engine = Engine::new("acme", "sprite-test");
    engine.set_physics(PhysicsWorld::none());
    engine.set_renderer(Box::new(renderer));

    // 4×4 texture, row-major: solid 2×2 quadrants (top row red/green,
    // bottom row blue/yellow) — quadrant-center samples stay inside a
    // solid block, so linear filtering can't bleed between colors
    let mut px = Vec::new();
    for y in 0..4u32 {
        for x in 0..4u32 {
            px.extend_from_slice(match (x / 2, y / 2) {
                (0, 0) => &[255, 0, 0, 255],
                (1, 0) => &[0, 255, 0, 255],
                (0, 1) => &[0, 0, 255, 255],
                _ => &[255, 255, 0, 255],
            });
        }
    }
    let tex = Texture::from_rgba(4, 4, px).expect("4×4 texture");
    let mut sprite = Entity::new("sprite");
    sprite.transform_mut().position = V3::new(0.0, 4.0, 0.0);
    sprite.add_component(Box::new(MeshRenderer::textured(
        std::sync::Arc::new(Mesh::quad(V3::new(1.0, 1.0, 0.0))),
        std::sync::Arc::new(tex),
        [1.0, 1.0, 1.0, 1.0],
    )));
    engine.add_entity(sprite);

    engine.add_entity(key_sun());
    engine.start_with(Box::new(HeadlessWindow::for_frames(1)));

    let frame = sink.latest().expect("no frame published");
    let shade = mesh_shade([0.0, 0.0, 1.0]);
    // quadrant centers (uv .25/.75), inside the solid blocks
    // top-left quadrant ← image top-left (red)
    assert_close(
        pixel(&frame.rgba, frame.width, 227, 122),
        [srgb8(srgb_decode(255) * shade), 0, 0, 255],
        "top-left texel",
    );
    // top-right ← image top-right (green)
    assert_close(
        pixel(&frame.rgba, frame.width, 253, 122),
        [0, srgb8(srgb_decode(255) * shade), 0, 255],
        "top-right texel",
    );
    // bottom-left ← image bottom-left (blue)
    assert_close(
        pixel(&frame.rgba, frame.width, 227, 148),
        [0, 0, srgb8(srgb_decode(255) * shade), 255],
        "bottom-left texel",
    );
    // bottom-right ← image bottom-right (yellow)
    assert_close(
        pixel(&frame.rgba, frame.width, 253, 148),
        [
            srgb8(srgb_decode(255) * shade),
            srgb8(srgb_decode(255) * shade),
            0,
            255,
        ],
        "bottom-right texel",
    );
}

/// The checker cube through the full engine pump: the +z face samples the
/// texture at the primitive's UVs — light block left of center, orange
/// block right of center, both under the key-light shade.
#[cfg(feature = "gltf")] // loads a glTF file (wgpu-only builds stay compilable)
#[tokio::test]
async fn gltf_checker_cube_samples_its_texture() {
    let sink = hyg::game::FrameSink::new();
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.attach_sink(sink.clone());
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);

    let mut engine = Engine::new("acme", "gltf-texture-test");
    engine.set_physics(PhysicsWorld::none());
    engine.set_renderer(Box::new(renderer));

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let model = Model::from_gltf_file(dir.join("cube_checker.gltf"))
        .expect("the checked-in checker cube loads");
    let mut cube = Entity::new("cube");
    cube.transform_mut().position = V3::new(0.0, 4.0, 0.0);
    cube.add_component(Box::new(MeshRenderer::model(
        std::sync::Arc::new(model),
        [1.0, 1.0, 1.0, 1.0],
    )));
    engine.add_entity(cube);

    engine.add_entity(key_sun());
    engine.start_with(Box::new(HeadlessWindow::for_frames(1)));

    let frame = sink.latest().expect("no frame published");
    let shade = mesh_shade([0.0, 0.0, 1.0]);
    // uv (0.25, 0.25) of the +z face → world (-0.25, 3.75): light block
    let light = [245u8, 240, 225];
    assert_close(
        pixel(&frame.rgba, frame.width, 233, 142),
        [
            srgb8(srgb_decode(light[0]) * shade),
            srgb8(srgb_decode(light[1]) * shade),
            srgb8(srgb_decode(light[2]) * shade),
            255,
        ],
        "light block",
    );
    // uv (0.75, 0.25) → world (0.25, 3.75): orange block
    let orange = [196u8, 90, 43];
    assert_close(
        pixel(&frame.rgba, frame.width, 247, 142),
        [
            srgb8(srgb_decode(orange[0]) * shade),
            srgb8(srgb_decode(orange[1]) * shade),
            srgb8(srgb_decode(orange[2]) * shade),
            255,
        ],
        "orange block",
    );
}

// ---- lights (gold's light component, KHR punctual shading) ----

/// A scene with no lights renders ambient-only (gold's zeroed light
/// contributes nothing): the front face lands exactly on the 0.45 floor.
#[tokio::test]
async fn no_lights_render_ambient_only() {
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
    renderer.begin_scene(
        &Camera::ortho(V3::new(0.0, 4.0, 0.0), V3::new(8.89, 5.0, 2.0)),
        &[],
    );
    renderer.push_model(
        &Transform {
            position: V3::new(0.0, 4.0, 0.0),
            ..Default::default()
        },
        &colored_cube([WHITE; 6]),
        WHITE,
    );
    renderer.render_frame();

    let frame = renderer.screenshot();
    let ambient = [srgb8(0.45); 3];
    assert_close(
        pixel(frame, 480, 240, 135),
        [ambient[0], ambient[1], ambient[2], 255],
        "ambient",
    );
}

/// Light direction drives shading: a directional light shining straight
/// onto the front face gives the full diffuse term (nl = 1), twice the
/// old key light's angled grazing.
#[tokio::test]
async fn directional_light_direction_matters() {
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
    renderer.begin_scene(
        &Camera::ortho(V3::new(0.0, 4.0, 0.0), V3::new(8.89, 5.0, 2.0)),
        // shines toward -z, so the +z face gets nl = 1
        &[SceneLight {
            position: V3::splat(0.0),
            direction: V3::new(0.0, 0.0, -1.0),
            kind: LightKind::Directional,
            color: [1.0, 1.0, 1.0],
            intensity: 0.55,
            cone: [1.0, 1.0],
            range: 0.0,
        }],
    );
    renderer.push_model(
        &Transform {
            position: V3::new(0.0, 4.0, 0.0),
            ..Default::default()
        },
        &colored_cube([WHITE; 6]),
        WHITE,
    );
    renderer.render_frame();

    let frame = renderer.screenshot();
    assert_close(
        pixel(frame, 480, 240, 135),
        [255, 255, 255, 255],
        "head-on light",
    );
    // the floor-level pixel: the front face ends at y = 4 − 0.8 → row 157
    let outside = (135.0 + 0.95 / 5.0 * 135.0) as u32;
    assert_close(
        pixel(frame, 480, 240, outside),
        [0, 0, 0, 255],
        "still outside the cube",
    );
}

/// Point lights fall off by the KHR punctual range attenuation:
/// (1 − (d/r)⁴)⁺ / d². A light 5 units in front of the face with range 10
/// contributes (1 − 0.5⁴)/25 ≈ 0.0375; just past the range it
/// contributes nothing and the face drops back to ambient.
#[tokio::test]
async fn point_light_falls_off_with_range() {
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
    // cube front face at z = 0.8; the light sits 5.0 units in front of it
    let camera = Camera::ortho(V3::new(0.0, 4.0, 0.0), V3::new(8.89, 5.0, 4.0));
    let cube_at = Transform {
        position: V3::new(0.0, 4.0, 0.0),
        ..Default::default()
    };
    let light_at = V3::new(0.0, 4.0, 5.8);

    renderer.begin_scene(
        &camera,
        &[SceneLight {
            position: light_at,
            direction: V3::new(0.0, 0.0, -1.0),
            kind: LightKind::Point,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            cone: [1.0, 1.0],
            range: 10.0,
        }],
    );
    renderer.push_model(&cube_at, &colored_cube([WHITE; 6]), WHITE);
    renderer.render_frame();

    // dist = 5, range = 10: (1 − (5/10)⁴)/5² = 0.9375/25 = 0.0375
    let frame = renderer.screenshot();
    let want = srgb8(0.45 + 0.0375);
    assert_close(
        pixel(frame, 480, 240, 135),
        [want, want, want, 255],
        "in-range point light",
    );

    // same light with range 4.9 (< dist): zero contribution → ambient
    renderer.begin_scene(
        &camera,
        &[SceneLight {
            position: light_at,
            direction: V3::new(0.0, 0.0, -1.0),
            kind: LightKind::Point,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            cone: [1.0, 1.0],
            range: 4.9,
        }],
    );
    renderer.push_model(&cube_at, &colored_cube([WHITE; 6]), WHITE);
    renderer.render_frame();

    let frame = renderer.screenshot();
    let ambient = srgb8(0.45);
    assert_close(
        pixel(frame, 480, 240, 135),
        [ambient, ambient, ambient, 255],
        "beyond the range",
    );
}

/// Spots gate by cone: gold's default cone (1, 1) collapses to nothing,
/// and a real cone lights what's inside it (on-axis here → full term).
#[tokio::test]
async fn spot_cone_gates_the_light() {
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(480, 270))
        .expect("no wgpu adapter on this machine");
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
    let camera = Camera::ortho(V3::new(0.0, 4.0, 0.0), V3::new(8.89, 5.0, 4.0));
    let cube_at = Transform {
        position: V3::new(0.0, 4.0, 0.0),
        ..Default::default()
    };

    // gold's default cone (1, 1): actualCos can never exceed 1 → dark
    renderer.begin_scene(
        &camera,
        &[SceneLight {
            position: V3::new(0.0, 4.0, 5.8),
            direction: V3::new(0.0, 0.0, -1.0),
            kind: LightKind::Spot,
            color: [1.0, 1.0, 1.0],
            intensity: 0.55,
            cone: [1.0, 1.0],
            range: 0.0,
        }],
    );
    renderer.push_model(&cube_at, &colored_cube([WHITE; 6]), WHITE);
    renderer.render_frame();

    let frame = renderer.screenshot();
    let ambient = srgb8(0.45);
    assert_close(
        pixel(frame, 480, 240, 135),
        [ambient, ambient, ambient, 255],
        "collapsed cone",
    );

    // inner 30° / outer 45° cones, light dead-on-axis: spot = 1
    renderer.begin_scene(
        &camera,
        &[SceneLight {
            position: V3::new(0.0, 4.0, 5.8),
            direction: V3::new(0.0, 0.0, -1.0),
            kind: LightKind::Spot,
            color: [1.0, 1.0, 1.0],
            intensity: 0.55,
            cone: [0.866, 0.707],
            range: 0.0,
        }],
    );
    renderer.push_model(&cube_at, &colored_cube([WHITE; 6]), WHITE);
    renderer.render_frame();

    let frame = renderer.screenshot();
    let lit = srgb8(0.45 + 0.55);
    assert_close(
        pixel(frame, 480, 240, 135),
        [lit, lit, lit, 255],
        "on-axis spot",
    );
}
