//! The editor's orbit camera — the viewport's own view, distinct from
//! the SCENE's camera (which the saved document and play mode carry).
//! Pure math: yaw/pitch/distance around a target produce a perspective
//! [`Camera`]; input deltas map to orbit/zoom/pan. Feature-free and
//! headless-tested; the shell feeds it pointer events.
//!
//! Controls (in the viewport): right-drag orbits, the wheel zooms,
//! middle-drag pans in the view plane. `Camera`'s right-handed look-at
//! does the rest.

use crate::game::camera::{Camera, Projection};
use crate::var::Vec3;

use super::gizmo::view_basis;

/// Orbit state: where the editor looks from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orbit {
    /// Radians around world +y (0 looks along +z from behind).
    pub yaw: f32,
    /// Radians above the horizon (clamped to ±~89°).
    pub pitch: f32,
    /// Distance from the target.
    pub distance: f32,
    /// The orbit's center (world).
    pub target: Vec3,
    /// Vertical fov (degrees) for the built camera.
    pub fov_y_degrees: f32,
    /// Orthographic view (the 2D editing mode): half-extents derive
    /// from the distance (zoom dollies the same knob either way).
    pub ortho: bool,
}

impl Default for Orbit {
    /// The sandbox's home view: above and behind the origin, looking
    /// down the +z axis the scenes are built along.
    fn default() -> Self {
        Orbit {
            ortho: false,
            // yaw π views from +z looking −z: world +x renders
            // screen-RIGHT — the gizmo convention every other editor
            // trains (yaw 0 views from behind +z and mirrors it)
            yaw: std::f32::consts::PI,
            pitch: 0.45,
            distance: 9.0,
            target: Vec3::new(0.0, 1.0, 0.0),
            fov_y_degrees: 60.0,
        }
    }
}

impl Orbit {
    /// The camera this orbit describes (aspect from the viewport — set
    /// by the shell each frame; near/far generous for editing). Positive
    /// pitch looks DOWN from above (the forward vector's y is -sin): a
    /// bird's-eye edit view, not a worm's-eye one.
    #[must_use]
    pub fn camera(&self, aspect: f32) -> Camera {
        let cos_p = self.pitch.cos();
        let forward = Vec3::new(
            self.yaw.sin() * cos_p,
            -self.pitch.sin(),
            self.yaw.cos() * cos_p,
        );
        let position = self.target - forward.scaled(Vec3::splat(self.distance));
        let projection = if self.ortho {
            // the same zoom knob: half-extents grow with distance
            let half_y = self.distance * (self.fov_y_degrees.to_radians() * 0.5).tan();
            Projection::Orthographic {
                half_extents: Vec3::new(half_y * aspect.max(0.1), half_y, 500.0),
            }
        } else {
            Projection::Perspective {
                fov_y_degrees: self.fov_y_degrees,
                near: 0.05,
                far: 500.0,
                aspect: aspect.max(0.1),
            }
        };
        Camera {
            position,
            target: self.target,
            projection,
        }
    }

    /// Orbit by pointer drag deltas (pixels): dragging right moves the
    /// EYE along the camera's CURRENT screen-right (view-invariant —
    /// `dEye/dyaw = screen_right·dist`), dragging up raises the eye.
    pub fn orbit_by(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * 0.01;
        self.pitch = (self.pitch + dy * 0.01).clamp(-1.53, 1.53);
    }

    /// Zoom by wheel delta (pixels): exponential in, clamped.
    pub fn zoom_by(&mut self, dy: f32) {
        let factor = (-dy * 0.0015).exp();
        self.distance = (self.distance * factor).clamp(0.5, 300.0);
    }

    /// Pan the target in the camera's view plane by pointer deltas
    /// (pixels) — scaled by distance so the grab follows the cursor at
    /// any zoom.
    pub fn pan_by(&mut self, camera: &Camera, dx: f32, dy: f32) {
        let (_, right, up) = view_basis(camera);
        let scale = self.distance * 0.0018;
        self.target = self.target
            + right.scaled(Vec3::splat(-dx * scale))
            + up.scaled(Vec3::splat(dy * scale));
    }
}

/// The editor's other camera: free-fly. The orbit orbits a target; the
/// fly is EYE-first — position, yaw/pitch heading, thrust along the
/// view axes — for walking through a scene at any height. The View menu
/// toggles them; toggling seeds the fly from the orbit's current pose.
///
/// Conventions match [`Orbit`]: yaw radians around world +y (yaw π
/// views from +z looking −z, world +x = screen right), pitch radians
/// above the horizon (positive = looking down), forward = (sin yaw·cos
/// p, −sin p, cos yaw·cos p).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fly {
    /// The eye (world).
    pub position: Vec3,
    /// Heading around world +y (radians).
    pub yaw: f32,
    /// Pitch above the horizon (radians, clamped to ±~89°).
    pub pitch: f32,
    /// Thrust speed: world units per second at key factor 1.
    pub speed: f32,
}

impl Default for Fly {
    /// Eye height over the default scene origin, looking −z (the orbit
    /// default's view direction, un-orphited).
    fn default() -> Self {
        Fly {
            position: Vec3::new(0.0, 2.0, 9.0),
            yaw: std::f32::consts::PI,
            pitch: 0.15,
            speed: 6.0,
        }
    }
}

impl Fly {
    /// Seed a fly from ANY camera's pose (the orbit → fly toggle keeps
    /// the eye and heading so the switch doesn't jump).
    #[must_use]
    pub fn from_camera(camera: &Camera) -> Fly {
        let forward = (camera.target - camera.position).normalize();
        let pitch = (-forward.y).clamp(-0.999, 0.999).asin();
        let flat = Vec3::new(forward.x, 0.0, forward.z);
        // forward = (sin yaw·cos p, −sin p, cos yaw·cos p): yaw =
        // atan2(x, z) (the same convention Orbit's yaw π default gives
        // forward −z)
        let yaw = if flat.dot(flat) > 1e-8 {
            flat.x.atan2(flat.z)
        } else {
            0.0
        };
        Fly {
            position: camera.position,
            yaw,
            pitch,
            speed: 6.0,
        }
    }

    /// Mouse-look: deltas in pixels, the orbit's feel (0.005 rad/px,
    /// pitch clamped before the poles).
    pub fn look(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * 0.005;
        self.pitch = (self.pitch + dy * 0.005).clamp(-1.53, 1.53);
    }

    /// Thrust: `forward`/`strafe`/`up` are key factors (−1..=1, strafe
    /// right-positive, up world-up-positive) applied for `dt` seconds
    /// at the fly's speed.
    pub fn thrust(&mut self, forward: f32, strafe: f32, up: f32, dt: f32) {
        let (sin_y, cos_y) = (self.yaw.sin(), self.yaw.cos());
        let (sin_p, cos_p) = (self.pitch.sin(), self.pitch.cos());
        let fwd = Vec3::new(sin_y * cos_p, -sin_p, cos_y * cos_p);
        // forward × world up: horizontal screen-right (at yaw π that's
        // world +x — the same convention the gizmo trains)
        let right = Vec3::new(-cos_y, 0.0, sin_y);
        let world_up = Vec3::new(0.0, 1.0, 0.0);
        let delta = fwd
            .scaled(Vec3::splat(forward))
            + right.scaled(Vec3::splat(strafe))
            + world_up.scaled(Vec3::splat(up));
        self.position = self.position + delta.scaled(Vec3::splat(self.speed * dt));
    }

    /// The perspective camera at this pose (target = eye + forward — a
    /// look-at down the heading).
    #[must_use]
    pub fn camera(&self, aspect: f32) -> Camera {
        let (sin_y, cos_y) = (self.yaw.sin(), self.yaw.cos());
        let (sin_p, cos_p) = (self.pitch.sin(), self.pitch.cos());
        let forward = Vec3::new(sin_y * cos_p, -sin_p, cos_y * cos_p);
        Camera {
            position: self.position,
            target: self.position + forward,
            projection: Projection::Perspective {
                fov_y_degrees: 60.0,
                near: 0.05,
                far: 500.0,
                aspect: aspect.max(0.1),
            },
        }
    }
}
