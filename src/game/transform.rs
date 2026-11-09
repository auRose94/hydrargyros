//! Transforms — position, rotation, scale, composed
//! down the entity hierarchy.

use crate::var::{Quat, Vec3};

/// A place in space: position, rotation, scale. Entities compose their
/// local transform with their parent's to get world transforms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Transform {
            position: Vec3::new(0.0, 0.0, 0.0),
            rotation: Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
            scale: Vec3::splat(1.0),
        }
    }
}

impl Transform {
    /// Identity transform.
    pub fn new() -> Self { Transform::default() }

    /// The world transform of something whose local transform is `self`
    /// under `parent`'s world transform: `parent.position + parent.rotation
    /// · (local.position * parent.scale)`, rotations multiply parent-first.
    #[must_use]
    pub fn compose(&self, parent: &Transform) -> Transform {
        Transform {
            position: parent.position + parent.rotation.rotate(self.position.scaled(parent.scale)),
            rotation: parent.rotation.compose(self.rotation),
            scale: Vec3 {
                x: parent.scale.x * self.scale.x,
                y: parent.scale.y * self.scale.y,
                z: parent.scale.z * self.scale.z,
            },
        }
    }
}
