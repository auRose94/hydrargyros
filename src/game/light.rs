//! Lights — the `light` component: prototype defaults `type: "point"`,
//! `color: 0xffffffff`, `intensity: 1.0`, `cone: vec2f(1, 1)`; shading
//! with the KHR_lights_punctual math.
//!
//! A light hangs off an entity: its **position** is the entity's world
//! origin and its **direction** is the entity's world forward (the
//! rotation applied to +z — the direction the light *shines*, KHR
//! convention). The engine gathers light components each draw pass and
//! hands them to the render backend; a scene with no lights renders
//! ambient-only (a zeroed light contributes nothing). The type strings:
//! `"point"` (default), `"directional"`, `"spot"` — `"area"` is not
//! supported. `cone` holds inner/outer cosines — the default `(1, 1)`
//! collapses a spot to nothing, so spots need real angles.

use super::component::{Component, Priority};
use super::transform::Transform;
use crate::var::Vec3;

/// What kind of light this is (the `type` string, as the renderer
/// encodes it: directional = 0, point = 1, spot = 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightKind {
    Directional,
    Point,
    Spot,
}

impl LightKind {
    /// The uniform encoding (float constants in the light uniform).
    pub fn as_u32(self) -> u32 {
        match self {
            LightKind::Directional => 0,
            LightKind::Point => 1,
            LightKind::Spot => 2,
        }
    }
}

/// A light component: color, intensity, cone, type. The
/// position/direction come from the owning entity's transform at draw
/// time, so lights move with entities like everything else.
#[derive(Debug, Clone)]
pub struct Light {
    pub kind: LightKind,
    /// Linear RGB, 0–1 per channel (each channel set directly, not
    /// packed).
    pub color: [f32; 3],
    pub intensity: f32,
    /// Inner/outer cone cosines for spots — `(1.0, 1.0)` by default,
    /// which collapses the cone to nothing.
    pub cone: [f32; 2],
    /// Distance falloff range for point/spot lights; `0.0` = unlimited
    /// (the uniform row contract: "range — 0 means unlimited").
    pub range: f32,
}

impl Default for Light {
    /// The prototype defaults: a white point light at intensity 1.
    fn default() -> Self {
        Light {
            kind: LightKind::Point,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            cone: [1.0, 1.0],
            range: 0.0,
        }
    }
}

impl Light {
    pub fn new() -> Self {
        Light::default()
    }

    /// A directional light (`type: "directional"`).
    pub fn directional() -> Self {
        Light {
            kind: LightKind::Directional,
            ..Light::default()
        }
    }

    /// A spot light (`type: "spot"`) — set real
    /// [`cone`](Self::cone) angles or it lights nothing.
    pub fn spot() -> Self {
        Light {
            kind: LightKind::Spot,
            ..Light::default()
        }
    }

    /// Set the color (builder).
    pub fn with_color(mut self, color: [f32; 3]) -> Self {
        self.color = color;
        self
    }

    /// Set the intensity (builder).
    pub fn with_intensity(mut self, intensity: f32) -> Self {
        self.intensity = intensity;
        self
    }

    /// Set the inner/outer cone cosines (builder).
    pub fn with_cone(mut self, cone: [f32; 2]) -> Self {
        self.cone = cone;
        self
    }

    /// Set the falloff range; `0.0` = unlimited (builder).
    pub fn with_range(mut self, range: f32) -> Self {
        self.range = range;
        self
    }
}

impl Component for Light {
    fn priority(&self) -> Priority {
        Priority::Generic
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn as_light(&self) -> Option<&Light> {
        Some(self)
    }
}

/// A light gathered from the scene: the component's config plus its
/// entity's world transform (position and forward direction resolved
/// from the light's world matrix at gather time).
#[derive(Debug, Clone, Copy)]
pub struct SceneLight {
    /// World position of the light's entity (point/spot origin).
    pub position: Vec3,
    /// World forward of the light's entity — the direction it shines.
    pub direction: Vec3,
    pub kind: LightKind,
    pub color: [f32; 3],
    pub intensity: f32,
    pub cone: [f32; 2],
    pub range: f32,
}

impl SceneLight {
    /// Resolve a light component at its entity's world transform.
    pub fn from_component(light: &Light, world: &Transform) -> SceneLight {
        SceneLight {
            position: world.position,
            direction: world.rotation.rotate(Vec3::new(0.0, 0.0, 1.0)),
            kind: light.kind,
            color: light.color,
            intensity: light.intensity,
            cone: light.cone,
            range: light.range,
        }
    }
}

/// A rotation taking +z — the light forward, the direction the light
/// *shines* — onto `dir` (normalized internally; degenerate inputs fall
/// back to identity). Light orientation belongs to the entity transform,
/// so "a sun shining toward `dir`" is an entity with this rotation and a
/// directional [`Light`].
pub fn forward_quat(dir: [f32; 3]) -> crate::var::Quat {
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len == 0.0 {
        return crate::var::Quat::new(0.0, 0.0, 0.0, 1.0);
    }
    let d = [dir[0] / len, dir[1] / len, dir[2] / len];
    let dot = d[2]; // dot(+z, d)
    if dot > 0.9999 {
        return crate::var::Quat::new(0.0, 0.0, 0.0, 1.0);
    }
    if dot < -0.9999 {
        return crate::var::Quat::new(1.0, 0.0, 0.0, 0.0); // 180° about +x
    }
    // axis = +z × d, angle = acos(dot)
    let axis = [-d[1], d[0], 0.0];
    let alen = (axis[0] * axis[0] + axis[1] * axis[1]).sqrt();
    let (ax, ay, az) = (axis[0] / alen, axis[1] / alen, 0.0);
    crate::var::Quat::axis_angle(Vec3::new(ax, ay, az), dot.acos())
}
