//! The camera — gold's `camera`: the world → clip transform for a 2D view.
//!
//! The camera is an orthographic window onto the world: `position` is the
//! world point at the center of the view, `half_extents` the half width and
//! height it covers (world units). The renderer transforms every quad
//! through it at draw time, so entities render in world coordinates and
//! the camera moves the view.
//!
//! ```
//! use hydrargyros::game::camera::Camera;
//! use hydrargyros::var::Vec3;
//!
//! // a 16:9 view centered at world (0, 4): y ∈ [-1, 9], x ∈ [-8.89, 8.89]
//! let camera = Camera {
//!     position: Vec3::new(0.0, 4.0, 0.0),
//!     half_extents: Vec3::new(8.89, 5.0, 1.0),
//! };
//! // the view center maps to the center of the screen
//! let c = camera.world_to_clip(Vec3::new(0.0, 4.0, 0.0));
//! assert_eq!(c, [0.0, 0.0]);
//! // world +y is screen up
//! let up = camera.world_to_clip(Vec3::new(0.0, 9.0, 0.0));
//! assert_eq!(up, [0.0, 1.0]);
//! ```

use crate::var::Vec3;

/// An orthographic 2D camera (gold's `camera`, v0: no rotation).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// World point at the center of the view.
    pub position: Vec3,
    /// Half width/height of the view, in world units. Match the aspect of
    /// the target surface for square pixels (16:9 surface → 16:10 vs 16:9
    /// half-extents ratio, e.g. `(8.89, 5.0)`). The `z` component is the
    /// **depth window** for meshes: they render between
    /// `position.z − z` (far) and `position.z + z` (near), and anything
    /// outside clips against those planes. Quads ignore it. Rotating
    /// models sweep corners through z — keep the window wider than the
    /// sweep (the default is generous for exactly that reason).
    pub half_extents: Vec3,
}

impl Default for Camera {
    /// A 16:9 view centered at world (0, 4) — y ∈ [-1, 9] — with a
    /// generous depth window (±64): meshes clip only when they truly
    /// leave the view, not because a rotating corner crossed a tight
    /// near plane.
    fn default() -> Self {
        Camera {
            position: Vec3::new(0.0, 4.0, 0.0),
            half_extents: Vec3::new(8.89, 5.0, 64.0),
        }
    }
}

impl Camera {
    /// World position → clip space (`[-1, 1]`²), +y up on the screen.
    #[must_use]
    pub fn world_to_clip(&self, pos: Vec3) -> [f32; 2] {
        [
            (pos.x - self.position.x) / self.half_extents.x,
            (pos.y - self.position.y) / self.half_extents.y,
        ]
    }
}
