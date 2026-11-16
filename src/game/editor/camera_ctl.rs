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
}

impl Default for Orbit {
    /// The sandbox's home view: above and behind the origin, looking
    /// down the +z axis the scenes are built along.
    fn default() -> Self {
        Orbit {
            yaw: 0.0,
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
        Camera {
            position,
            target: self.target,
            projection: Projection::Perspective {
                fov_y_degrees: self.fov_y_degrees,
                near: 0.05,
                far: 500.0,
                aspect: aspect.max(0.1),
            },
        }
    }

    /// Orbit by pointer drag deltas (pixels): dragging right moves the
    /// EYE right around the target (the scene slides left — the standard
    /// editor orbit), dragging up raises the eye. The eye sits at
    /// `target − forward·dist`, so the yaw sign runs against the drag.
    pub fn orbit_by(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.01;
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
