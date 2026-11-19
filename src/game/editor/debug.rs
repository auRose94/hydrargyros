//! The dock's debug views' data: READ-ONLY snapshots of the physics and
//! audio worlds, built per frame from the engine's state and rendered by
//! the shell's dock tabs. Snapshot-building is pure data in → data out —
//! it runs and tests headless; the views are egui and stay in the shell.
//!
//! The physics rows are the backend's own records
//! ([`super::super::physics::PhysicsWorldBackend::bodies`]) — the
//! simulation stays the backend's, the view only reads what was
//! declared. The audio rows ride the none recorder's introspection seam
//! (`AudioWorld::none_plays`); real backends add their own records
//! alongside later — the snapshot's `plays` is optional for exactly
//! that reason.

use crate::game::physics::BodyInfo;
use crate::var::Vec3;

/// One frame's physics view: the backend's name, its gravity (when it
/// tracks one), and the bodies as declared.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicsSnapshot {
    pub backend: String,
    pub gravity: Option<Vec3>,
    pub bodies: Vec<BodyInfo>,
}

impl PhysicsSnapshot {
    /// Snapshot a physics world (per frame; the fields are cheap clones
    /// of the backend's records).
    #[must_use]
    pub fn from_world(world: &crate::game::physics::PhysicsWorld) -> PhysicsSnapshot {
        PhysicsSnapshot {
            backend: world.backend_name().to_string(),
            gravity: world.gravity(),
            bodies: world.bodies(),
        }
    }
}

/// One frame's audio view. Feature-gated with the `audio` module: the
/// whole snapshot is meaningless without a backend to introspect.
#[cfg(feature = "audio")]
#[derive(Debug, Clone, PartialEq)]
pub struct AudioSnapshot {
    pub backend: String,
    /// The master volume the backend reports (`None` = the backend
    /// doesn't report one — the view shows its slider at the default).
    pub master_volume: Option<f32>,
    /// The recorder's play records (`None` = the backend keeps no play
    /// list — real output backends may not; the view shows "no play
    /// records" rather than an empty claim).
    pub plays: Option<Vec<crate::game::audio::NonePlay>>,
}

#[cfg(feature = "audio")]
impl AudioSnapshot {
    /// Snapshot an audio world.
    #[must_use]
    pub fn from_world(world: &crate::game::audio::AudioWorld) -> AudioSnapshot {
        AudioSnapshot {
            backend: world.backend_name().to_string(),
            master_volume: world.none_master_volume(),
            plays: world.none_plays(),
        }
    }
}
