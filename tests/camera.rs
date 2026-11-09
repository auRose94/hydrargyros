//! Camera 3D tests: the `view_projection` matrix (the ortho matrix pinned
//! bit-for-bit, the perspective look-at + fov with the WebGPU z convention)
//! and the Hydrargyros-native orbit controls over synthetic window events.

use hyg::game::camera::{Camera, OrbitController, Projection};
use hyg::o;
use hyg::var::Vec3;
use hyg::Var;

/// Multiply a column-major 4×4 by a world point → the raw clip vector.
fn transform(m: &[[f32; 4]; 4], p: Vec3) -> [f32; 4] {
    let v = [p.x, p.y, p.z, 1.0];
    let mut out = [0.0f32; 4];
    for (r, cell) in out.iter_mut().enumerate() {
        *cell = (0..4).map(|c| m[c][r] * v[c]).sum();
    }
    out
}

fn ndc(clip: [f32; 4]) -> [f32; 2] {
    [clip[0] / clip[3], clip[1] / clip[3]]
}

/// The perspective defaults: fov 60° y, near 0.1, far 100, 16:9 — eye at
/// the origin looking down +z.
fn perspective() -> Camera {
    Camera {
        position: Vec3::splat(0.0),
        target: Vec3::new(0.0, 0.0, 1.0),
        projection: Projection::Perspective {
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 100.0,
            aspect: 16.0 / 9.0,
        },
    }
}

// ---- orthographic: the v0 matrix, bit-for-bit ----

/// The default ortho camera reproduces the matrix `render_wgpu`'s
/// `camera_matrix` always built (now `Camera::view_projection`'s ortho
/// arm) — every pre-existing pixel test keeps its exact mapping.
#[test]
fn ortho_view_projection_matches_the_v0_matrix() {
    // default: position (0, 4, 0), half-extents (8.89, 5, 64)
    let m = Camera::default().view_projection();
    assert_eq!(m[0], [1.0 / 8.89, 0.0, 0.0, 0.0]);
    assert_eq!(m[1], [0.0, 1.0 / 5.0, 0.0, 0.0]);
    assert_eq!(m[2], [0.0, 0.0, -1.0 / (2.0 * 64.0), 0.0]);
    assert_eq!(
        m[3],
        [-0.0 / 8.89, -4.0 / 5.0, 0.5 + 0.0 / (2.0 * 64.0), 1.0]
    );
}

/// The ortho matrix agrees with [`Camera::world_to_clip`] on x/y, and the
/// depth window maps nearest → 0, farthest → 1 (WebGPU's z convention).
#[test]
fn ortho_matrix_agrees_with_world_to_clip() {
    let camera = Camera::ortho(Vec3::new(0.0, 4.0, 0.0), Vec3::new(8.0, 4.5, 1.0));
    let p = Vec3::new(2.0, 5.5, 0.25);
    let clip = transform(&camera.view_projection(), p);
    let [x, y] = camera.world_to_clip(p);
    // (the matrix multiplies out in a different op order than the direct
    // division — same value to f32 rounding, not to the bit)
    assert!((clip[0] - x).abs() < 1e-6);
    assert!((clip[1] - y).abs() < 1e-6);
    assert_eq!(clip[3], 1.0);
    // depth window: z ∈ [position.z − 1, position.z + 1] → [0, 1]
    let nearest = transform(&camera.view_projection(), Vec3::new(0.0, 4.0, 1.0));
    let farthest = transform(&camera.view_projection(), Vec3::new(0.0, 4.0, -1.0));
    assert_eq!(nearest[2], 0.0);
    assert_eq!(farthest[2], 1.0);
}

// ---- perspective: the look-at + projection matrix ----

/// A point straight ahead of the eye lands at the clip center, and w is
/// the view depth (the projection's w row selects view z: clip.w = view.z).
#[test]
fn perspective_maps_ahead_points_to_the_clip_center() {
    let camera = perspective();
    let m = camera.view_projection();
    let clip = transform(&m, Vec3::new(0.0, 0.0, 4.0)); // eye + 4·forward
    assert_eq!(ndc(clip), [0.0, 0.0]);
    assert!((clip[3] - 4.0).abs() < 1e-6, "w = view z, got {}", clip[3]);

    // off-axis: y scales by 1/tan(fov/2), x by that over the aspect, both
    // divided by depth (the x sign follows the right-handed view frame —
    // this rig looks +z, so screen-right is world −x)
    let yscale = 1.0 / (60.0f32.to_radians() * 0.5).tan();
    let xscale = yscale / (16.0f32 / 9.0);
    let clip = transform(&m, Vec3::new(0.5, 0.25, 4.0));
    let [x, y] = ndc(clip);
    assert!((x.abs() - xscale * 0.5 / 4.0).abs() < 1e-5);
    assert!((y - yscale * 0.25 / 4.0).abs() < 1e-5);
}

/// Handedness: the perspective view is bx's RIGHT-handed look-at (view
/// space −z toward the target — the module doc records why), so the
/// screen-right axis is up × (eye − target): world −x is screen-right for
/// this +z-looking rig, +y is screen-up. Both cameras' frames are proper
/// rotations, so no view direction mirrors the image.
#[test]
fn perspective_look_at_axes() {
    let camera = perspective();
    let m = camera.view_projection();
    let right = ndc(transform(&m, Vec3::new(-1.0, 0.0, 1.0)));
    let left = ndc(transform(&m, Vec3::new(1.0, 0.0, 1.0)));
    let up = ndc(transform(&m, Vec3::new(0.0, 1.0, 1.0)));
    assert!(right[0] > 0.0, "world −x → screen right, got {right:?}");
    assert!(left[0] < 0.0, "world +x → screen left, got {left:?}");
    assert!(up[1] > 0.0, "world +y → screen up, got {up:?}");
}

/// The WebGPU z trap: a point AT the near plane maps to clip z 0, a point
/// at the far plane to z = w (z ∈ [0, 1] after the divide — bx's
/// homo=false, not GL's [-1, 1]; a GL-style mapping would clip the entire
/// near half).
#[test]
fn perspective_z_is_zero_at_the_near_plane_and_one_at_the_far() {
    let camera = perspective();
    let m = camera.view_projection();

    let near = transform(&m, Vec3::new(0.0, 0.0, 0.1)); // eye + near·forward
    assert!(near[2] == 0.0, "near-plane clip z, got {}", near[2]);
    assert!((near[2] / near[3]).abs() < 1e-7);

    let far = transform(&m, Vec3::new(0.0, 0.0, 100.0)); // eye + far·forward
    assert!(
        (far[2] / far[3] - 1.0).abs() < 1e-5,
        "far-plane z/w, got {}",
        far[2] / far[3]
    );

    // between the planes: strictly inside (0, 1)
    let mid = transform(&m, Vec3::new(0.0, 0.0, 50.0));
    let z = mid[2] / mid[3];
    assert!(z > 0.0 && z < 1.0, "mid z/w = {z}");
}

/// Points behind the eye get w ≤ 0 (outside the clip volume — the GPU
/// discards them).
#[test]
fn points_behind_the_eye_have_nonpositive_w() {
    let camera = perspective();
    let behind = transform(&camera.view_projection(), Vec3::new(0.0, 0.0, -2.0));
    assert!(behind[3] <= 0.0, "w behind the eye, got {}", behind[3]);
}

/// Foreshortening: the same world offset covers MORE NDC at the nearer
/// depth (a same-size face at z=2 spans wider than one at z=4).
#[test]
fn nearer_same_size_extent_covers_more_ndc() {
    let camera = perspective();
    let m = camera.view_projection();
    let near = ndc(transform(&m, Vec3::new(0.5, 0.0, 2.0)));
    let far = ndc(transform(&m, Vec3::new(0.5, 0.0, 4.0)));
    assert!(near[0].abs() > far[0].abs(), "near {near:?} vs far {far:?}");
}

// ---- orbit controls (Hydrargyros-native) ----

fn mouse(action: &str, button: i64, x: f64, y: f64) -> Var {
    Var::from(o! { "type": "mouse", "action": action, "button": button, "x": x, "y": y })
}

fn wheel(delta: f64) -> Var {
    Var::from(o! { "type": "wheel", "delta": delta, "x": 0.0, "y": 0.0 })
}

/// Left-drag orbits: down arms, moves swing yaw/pitch by dx/yaw·sensitivity
/// (drag right = scene right, drag down = eye up), up disarms.
#[test]
fn orbit_drags_swing_yaw_and_pitch() {
    let mut orbit = OrbitController::new(Vec3::splat(0.0));
    let (yaw0, pitch0) = (orbit.yaw, orbit.pitch);
    orbit.handle(&mouse("down", 0, 100.0, 100.0));
    orbit.handle(&mouse("move", 0, 140.0, 90.0)); // dx = 40, dy = −10
    assert!((orbit.yaw - (yaw0 - 40.0 * orbit.sensitivity)).abs() < 1e-5);
    assert!((orbit.pitch - (pitch0 + (-10.0) * orbit.sensitivity)).abs() < 1e-5);
    // up disarms: further moves do nothing
    orbit.handle(&mouse("up", 0, 200.0, 200.0));
    orbit.handle(&mouse("move", 0, 240.0, 40.0));
    assert!((orbit.yaw - (yaw0 - 40.0 * orbit.sensitivity)).abs() < 1e-5);
    assert!((orbit.pitch - (pitch0 - 10.0 * orbit.sensitivity)).abs() < 1e-5);
}

/// A move with no button down (disarmed) is a no-op — hover doesn't orbit.
#[test]
fn disarmed_moves_do_nothing() {
    let mut orbit = OrbitController::new(Vec3::splat(0.0));
    let before = orbit;
    orbit.handle(&mouse("move", 0, 300.0, 300.0));
    assert_eq!(orbit, before);
}

/// Pitch clamps at ±89° (never flips over the poles) and the distance
/// never drops under MIN_DISTANCE.
#[test]
fn orbit_clamps_pitch_and_distance() {
    let mut orbit = OrbitController::new(Vec3::splat(0.0));
    orbit.handle(&mouse("down", 0, 0.0, 0.0));
    for i in 1..=30i64 {
        // +100px further down each move (the delta is per-move)
        orbit.handle(&mouse("move", 0, 0.0, (100 * i) as f64));
    }
    assert_eq!(orbit.pitch, OrbitController::MAX_PITCH);
    for i in 1..=60i64 {
        // and back up, then over the top
        orbit.handle(&mouse("move", 0, 0.0, (3000 - 100 * i) as f64));
    }
    assert_eq!(orbit.pitch, -OrbitController::MAX_PITCH);
    orbit.handle(&mouse("up", 0, 0.0, 0.0));

    // zooming in hard bottoms out at MIN_DISTANCE (negative delta = in)
    orbit.handle(&wheel(-10_000.0));
    assert_eq!(orbit.distance, OrbitController::MIN_DISTANCE);
}

/// Non-left buttons never arm the orbit (winit Right=1/Middle=2, DOM
/// 1=middle/2=right — orbit only ever uses left, button 0).
#[test]
fn only_the_left_button_arms() {
    let mut orbit = OrbitController::new(Vec3::splat(0.0));
    let before = orbit;
    orbit.handle(&mouse("down", 1, 10.0, 10.0));
    orbit.handle(&mouse("down", 2, 10.0, 10.0));
    orbit.handle(&mouse("move", 1, 50.0, 50.0));
    assert_eq!(orbit, before);
}

/// Wheel scales the distance multiplicatively: negative delta (DOM wheel
/// up) zooms in, positive zooms out.
#[test]
fn wheel_zooms() {
    let mut orbit = OrbitController::new(Vec3::splat(0.0));
    let d0 = orbit.distance;
    orbit.handle(&wheel(-100.0)); // a DOM notch: 4 · (1 − 100·0.001) = 3.6
    assert!((orbit.distance - d0 * 0.9).abs() < 1e-5);
    orbit.handle(&wheel(100.0)); // and back out (multiplicative: d0·0.9·1.1)
    assert!((orbit.distance - d0 * 0.9 * 1.1).abs() < 1e-5);
}

/// `camera()` puts the eye on the sphere around the target:
/// `target + d·(cos(pitch)sin(yaw), sin(pitch), cos(pitch)cos(yaw))`,
/// looking at the target with the standard fov/near/far.
#[test]
fn orbit_camera_matches_the_spherical_formula() {
    let target = Vec3::new(1.0, 2.0, 3.0);
    let mut orbit = OrbitController::new(target);
    orbit.yaw = 30.0;
    orbit.pitch = 20.0;
    orbit.distance = 4.0;

    let camera = orbit.camera();
    let (yaw, pitch) = (30.0f32.to_radians(), 20.0f32.to_radians());
    let eye = target
        + Vec3::new(
            4.0 * pitch.cos() * yaw.sin(),
            4.0 * pitch.sin(),
            4.0 * pitch.cos() * yaw.cos(),
        );
    assert!(
        (camera.position - eye).length() < 1e-5,
        "{:?} vs {eye:?}",
        camera.position
    );
    assert_eq!(camera.target, target);
    // the eye sits exactly `distance` from the target
    assert!(((camera.position - camera.target).length() - 4.0).abs() < 1e-5);
    assert_eq!(
        camera.projection,
        Projection::Perspective {
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 100.0,
            aspect: 16.0 / 9.0,
        }
    );
}
