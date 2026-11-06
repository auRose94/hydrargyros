//! The camera — gold's `camera`: the world → clip transform for a view.
//!
//! gold's camera is perspective-only: prototype defaults `fov` 60
//! (degrees, y axis), `near` 0.1, `far` 100, with `size` set from the
//! window → `ratio = width/height`; every frame it looks along its
//! transform's +z (`eye = world·(0,0,0,1)`, `at = world·(0,0,1,1)`) and
//! uploads `lookAt(eye, at)` + `projection(fov, ratio, near, far,
//! homo=false)` (camera.cpp `setViewTransform`). The bx math is row-major;
//! this port is column-major for WGSL — the transpose duality gold's own
//! meshRenderer comment notes. One divergence, on purpose: gold's look-at
//! is left-handed (bx's default, view-space +z toward the target), but the
//! mesh pipeline's back-face winding was established by the v0 ortho
//! camera, whose sight is GL-style −z — so the perspective view uses bx's
//! **right-handed** variant (view-space −z toward the target) and ONE cull
//! mode stays correct for both cameras. `homo=false` is kept either way:
//! z ∈ [0, 1] after the divide — exactly WebGPU's clip z ∈ [0, w]
//! convention, so `CompareFunction::Less` keeps meaning "closer wins".
//!
//! Hydrargyros keeps the v0 orthographic window (the original 2D camera)
//! as a second mode: quads stay an orthographic image of the world — a
//! deliberate divergence from gold, which projects everything through the
//! one camera. Under a perspective camera the quad scene uses the window
//! derived at the target plane ([`Camera::quad_window`]), so flat scene
//! furniture lines up with the 3D view at the target's depth.
//!
//! [`OrbitController`] is Hydrargyros-native (gold has no orbit controls —
//! its camera rides the script seam): yaw/pitch/distance around a target,
//! fed the unified mouse/wheel `Var` events, building the perspective
//! [`Camera`] each frame.
//!
//! ```
//! use hydrargyros::game::camera::{Camera, Projection};
//! use hydrargyros::var::Vec3;
//!
//! // gold's defaults: fov 60° (y), near 0.1, far 100, 16:9
//! let camera = Camera {
//!     position: Vec3::new(0.0, 0.0, -4.0),
//!     target: Vec3::splat(0.0),
//!     projection: Projection::Perspective {
//!         fov_y_degrees: 60.0,
//!         near: 0.1,
//!         far: 100.0,
//!         aspect: 16.0 / 9.0,
//!     },
//! };
//! // the target (eye + 4·forward) lands at the clip center
//! let m = camera.view_projection();
//! let (x, y, z, w) = (0.0, 0.0, 0.0, 1.0); // the look-at point
//! let clip: Vec<f32> = (0..4)
//!     .map(|r| m[0][r] * x + m[1][r] * y + m[2][r] * z + m[3][r] * w)
//!     .collect();
//! assert_eq!(clip[0] / clip[3], 0.0);
//! assert_eq!(clip[1] / clip[3], 0.0);
//! ```

use crate::var::Vec3;

/// The projection mode (gold's camera is perspective-only; the
/// orthographic arm is Hydrargyros's v0 2D window, kept for the quad
/// scene).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Projection {
    /// v0: the orthographic window (the original `Camera` semantics, moved
    /// in here). `position` is the view center, `half_extents` the half
    /// width/height it covers; `half_extents.z` is the **mesh depth
    /// window** (nearest plane `position.z + z`, farthest `position.z −
    /// z`). `target` is ignored, as in v0.
    Orthographic {
        /// Half width/height of the view, in world units. Match the aspect
        /// of the target surface for square pixels (16:9 surface → e.g.
        /// `(8.89, 5.0)`).
        half_extents: Vec3,
    },
    /// gold's camera: fov in degrees (y axis) — gold's defaults 60/0.1/100
    /// — and aspect = width/height (gold's `ratio`, set by the embedder;
    /// resize-aware aspect is a later refinement).
    Perspective {
        /// Vertical field of view in degrees (gold's `fov`).
        fov_y_degrees: f32,
        /// Near clip distance (gold's `near`).
        near: f32,
        /// Far clip distance (gold's `far`).
        far: f32,
        /// Viewport width/height (gold's `ratio`).
        aspect: f32,
    },
}

/// The world → clip transform for a view: the eye/center `position`, the
/// look-at `target`, and the [`Projection`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// The eye (perspective) or the view center (orthographic).
    pub position: Vec3,
    /// The look-at point (perspective). The orthographic v0 camera ignored
    /// it and still does.
    pub target: Vec3,
    pub projection: Projection,
}

impl Default for Camera {
    /// The v0 orthographic default, bit-for-bit: a 16:9 view centered at
    /// world (0, 4) — y ∈ [-1, 9] — with a generous depth window (±64):
    /// meshes clip only when they truly leave the view, not because a
    /// rotating corner crossed a tight near plane.
    fn default() -> Self {
        Camera {
            position: Vec3::new(0.0, 4.0, 0.0),
            target: Vec3::splat(0.0),
            projection: Projection::Orthographic {
                half_extents: Vec3::new(8.89, 5.0, 64.0),
            },
        }
    }
}

impl Camera {
    /// The v0 orthographic camera as a constructor: view center +
    /// half-extents (the shape the original `Camera` literal had).
    #[must_use]
    pub fn ortho(position: Vec3, half_extents: Vec3) -> Self {
        Camera {
            position,
            target: Vec3::splat(0.0),
            projection: Projection::Orthographic { half_extents },
        }
    }

    /// World position → clip space (`[-1, 1]`²), +y up on the screen — the
    /// quad scene's mapping: an orthographic image of the world through
    /// [`Camera::quad_window`], centered on `position` (ortho) or on the
    /// target (perspective — the look-at point is the view's center).
    #[must_use]
    pub fn world_to_clip(&self, pos: Vec3) -> [f32; 2] {
        let half = self.quad_window();
        let center = match self.projection {
            Projection::Orthographic { .. } => self.position,
            Projection::Perspective { .. } => self.target,
        };
        [(pos.x - center.x) / half.x, (pos.y - center.y) / half.y]
    }

    /// The orthographic window the QUAD scene maps through. Orthographic →
    /// the mode's half-extents (x, y the view, z the mesh depth window).
    /// Perspective → a derived window at the target plane: half_y =
    /// |target − position|·tan(fov/2), half_x = half_y·aspect, z a
    /// generous depth window (far·2). Divergence from gold, on purpose:
    /// gold projects everything through the one camera; here the 2D quad
    /// scene stays an orthographic image of the world, sized to line up
    /// with the perspective view at the target's depth (quads at other
    /// depths won't match — they're flat scene furniture, not geometry).
    #[must_use]
    pub fn quad_window(&self) -> Vec3 {
        match self.projection {
            Projection::Orthographic { half_extents } => half_extents,
            Projection::Perspective {
                fov_y_degrees,
                aspect,
                far,
                ..
            } => {
                let half_y = (self.target - self.position).length()
                    * (fov_y_degrees.to_radians() * 0.5).tan();
                Vec3::new(half_y * aspect, half_y, far * 2.0)
            }
        }
    }

    /// The full world → clip matrix, column-major (WGSL's order — gold's
    /// bx matrices are row-major; the transpose duality gold's meshRenderer
    /// comment notes). Orthographic → the v0 window mapping (the matrix
    /// `render_wgpu`'s `camera_matrix` always built). Perspective → gold's
    /// camera: `lookAt(eye, at)` with the eye at `position` looking at
    /// `target` (up (0,1,0), right-handed — view-space −z toward the
    /// target; see the module doc for the handedness divergence), then bx's
    /// `mtxProj(fov, aspect, near, far, homo=false)` — z ∈ [0, 1] after the
    /// divide, WebGPU's clip z ∈ [0, w] convention.
    #[must_use]
    pub fn view_projection(&self) -> [[f32; 4]; 4] {
        match self.projection {
            Projection::Orthographic { half_extents } => {
                let hx = half_extents.x.max(1e-6);
                let hy = half_extents.y.max(1e-6);
                let hz = half_extents.z.max(1e-6);
                [
                    [1.0 / hx, 0.0, 0.0, 0.0],
                    [0.0, 1.0 / hy, 0.0, 0.0],
                    [0.0, 0.0, -1.0 / (2.0 * hz), 0.0],
                    [
                        -self.position.x / hx,
                        -self.position.y / hy,
                        0.5 + self.position.z / (2.0 * hz),
                        1.0,
                    ],
                ]
            }
            Projection::Perspective {
                fov_y_degrees,
                near,
                far,
                aspect,
            } => {
                // bx mtxLookAt with Handedness::Right: view =
                // normalize(eye − at) — view-space −z points at the target
                // (GL-style). gold's camera is left-handed (bx's default),
                // but the mesh pipeline's winding was established by the v0
                // ortho camera, whose sight is −z too: the right-handed
                // variant keeps ONE cull mode correct for both cameras (the
                // left-handed one renders meshes inside-out — far faces
                // pass cull instead of near ones; pixel tests catch it).
                let eye = self.position;
                let view = (eye - self.target).normalize();
                let up = Vec3::new(0.0, 1.0, 0.0);
                // Looking straight down the up axis is degenerate — bx
                // pins right to (1, 0, 0) for the right-handed look.
                let uxv = up.cross(view);
                let right = if uxv.dot(uxv) == 0.0 {
                    Vec3::new(1.0, 0.0, 0.0)
                } else {
                    uxv.normalize()
                };
                let up = view.cross(right);
                // bx's rows [right; up; view; translation], transposed into
                // column-major
                let view_m = [
                    [right.x, up.x, view.x, 0.0],
                    [right.y, up.y, view.y, 0.0],
                    [right.z, up.z, view.z, 0.0],
                    [-right.dot(eye), -up.dot(eye), -view.dot(eye), 1.0],
                ];
                // bx mtxProj(fovy, aspect, near, far, homo=false),
                // right-handed: [11] = −1 puts clip.w = −view.z (positive
                // in front) and z ∈ [0, 1] after the divide — WebGPU's clip
                // z ∈ [0, w] convention (a GL-style [−1, 1] mapping would
                // clip the entire near half instead)
                let height = 1.0 / (fov_y_degrees.to_radians() * 0.5).tan();
                let width = height / aspect;
                let aa = far / (far - near);
                let bb = near * aa;
                let proj = [
                    [width, 0.0, 0.0, 0.0],
                    [0.0, height, 0.0, 0.0],
                    [0.0, 0.0, -aa, -1.0],
                    [0.0, 0.0, -bb, 0.0],
                ];
                mul(&proj, &view_m)
            }
        }
    }
}

/// Column-major 4×4 multiply: `a · b` — apply `b` first (column vectors,
/// WGSL's `a * b`).
fn mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for (c, out_col) in out.iter_mut().enumerate() {
        for (r, cell) in out_col.iter_mut().enumerate() {
            *cell = a[0][r] * b[c][0] + a[1][r] * b[c][1] + a[2][r] * b[c][2] + a[3][r] * b[c][3];
        }
    }
    out
}

/// Hydrargyros-native orbit controls (gold has none — its camera rides the
/// script seam): yaw/pitch/distance around a target, fed the unified
/// window events (`WindowSystem::poll` `Var`s), building the perspective
/// [`Camera`] each frame. Angles are degrees (gold's fov is degrees too).
///
/// Drag = left button (button 0 — winit and the DOM agree left is 0; they
/// diverge on right/middle: winit Right=1/Middle=2, DOM 1=middle/2=right —
/// orbit only ever uses left). Signs: dragging right orbits the scene
/// right (`yaw -= dx·sensitivity` — the camera swings left around the
/// target, the scene turns right on screen); dragging down raises the eye
/// (`pitch += dy·sensitivity` — you see more of the top). Wheel: negative
/// delta (DOM wheel up) zooms in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitController {
    /// The point the camera orbits and looks at.
    pub target: Vec3,
    /// Orbit angle around the target's y axis, degrees.
    pub yaw: f32,
    /// Orbit elevation, degrees — clamped to ±89 so the eye never flips
    /// over the top.
    pub pitch: f32,
    /// Eye distance from the target — clamped above [`OrbitController::MIN_DISTANCE`].
    pub distance: f32,
    /// Degrees of orbit per pixel of drag.
    pub sensitivity: f32,
    /// Fraction of `distance` added per unit of wheel delta (DOM deltas
    /// run ~±100 per notch, winit's ~±10).
    pub zoom_speed: f32,
    /// Viewport width/height for the projection (gold's `ratio`, set by
    /// the embedder).
    pub aspect: f32,
    /// True between left-button down and up (a drag is in progress).
    armed: bool,
    /// Last cursor position while armed (pixels).
    last: (f64, f64),
}

impl OrbitController {
    /// gold's camera prototype defaults (fov 60° y, near 0.1, far 100).
    pub const FOV_Y_DEGREES: f32 = 60.0;
    pub const NEAR: f32 = 0.1;
    pub const FAR: f32 = 100.0;
    /// The eye never gets closer than this to the target (world units).
    pub const MIN_DISTANCE: f32 = 0.01;
    /// Pitch never passes the poles (±89°).
    pub const MAX_PITCH: f32 = 89.0;

    /// A third-person angle on the target: yaw 30°, pitch 20°, distance 4
    /// — gold's fov/near/far, 16:9 aspect.
    #[must_use]
    pub fn new(target: Vec3) -> Self {
        OrbitController {
            target,
            ..Default::default()
        }
    }

    /// Consume one unified window event: left-button down arms the orbit,
    /// move-while-armed orbits (`yaw -= dx·sens; pitch += dy·sens`), up
    /// disarms, wheel scales the distance (negative delta zooms in — the
    /// DOM convention). Everything else is ignored.
    pub fn handle(&mut self, event: &crate::var::Var) {
        match event.get("type").as_str() {
            Some("mouse") => {
                let x = event.get("x").as_f64().unwrap_or(self.last.0);
                let y = event.get("y").as_f64().unwrap_or(self.last.1);
                match event.get("action").as_str() {
                    // button 0 = left in both backends (see type doc)
                    Some("down") if event.get("button").as_i64() == Some(0) => {
                        self.armed = true;
                        self.last = (x, y);
                    }
                    Some("up") => self.armed = false,
                    Some("move") if self.armed => {
                        let dx = (x - self.last.0) as f32;
                        let dy = (y - self.last.1) as f32;
                        self.last = (x, y);
                        self.yaw -= dx * self.sensitivity;
                        self.pitch = (self.pitch + dy * self.sensitivity)
                            .clamp(-Self::MAX_PITCH, Self::MAX_PITCH);
                    }
                    _ => {}
                }
            }
            Some("wheel") => {
                if let Some(delta) = event.get("delta").as_f64() {
                    // negative delta = wheel up = zoom in (DOM convention)
                    self.distance = (self.distance * (1.0 + delta as f32 * self.zoom_speed))
                        .max(Self::MIN_DISTANCE);
                }
            }
            _ => {}
        }
    }

    /// The perspective camera at this orbit's pose: the eye on the sphere
    /// `target + distance·(cos(pitch)·sin(yaw), sin(pitch),
    /// cos(pitch)·cos(yaw))` — yaw 0 puts the eye on the target's +z, pitch
    /// raises it — looking at the target, gold's fov/near/far defaults.
    #[must_use]
    pub fn camera(&self) -> Camera {
        let (yaw, pitch) = (self.yaw.to_radians(), self.pitch.to_radians());
        Camera {
            position: self.target
                + Vec3::new(
                    self.distance * pitch.cos() * yaw.sin(),
                    self.distance * pitch.sin(),
                    self.distance * pitch.cos() * yaw.cos(),
                ),
            target: self.target,
            projection: Projection::Perspective {
                fov_y_degrees: Self::FOV_Y_DEGREES,
                near: Self::NEAR,
                far: Self::FAR,
                aspect: self.aspect,
            },
        }
    }
}

impl Default for OrbitController {
    fn default() -> Self {
        OrbitController {
            target: Vec3::splat(0.0),
            yaw: 30.0,
            pitch: 20.0,
            distance: 4.0,
            sensitivity: 0.25,
            zoom_speed: 0.001,
            aspect: 16.0 / 9.0,
            armed: false,
            last: (0.0, 0.0),
        }
    }
}
