//! Transform gizmos — the screen-space math behind the viewport's
//! translate / rotate / scale handles. Pure data and camera math: no
//! egui, no wgpu, so the layout, hit-testing, and drag deltas all run
//! and test headless. The shell paints what this module lays out.
//!
//! The gizmo is sized in PIXELS, not world units (a handle that stays
//! grabbable at any distance); each axis's px-per-world-unit factor is
//! measured at the entity's depth, so a drag maps screen motion back to
//! honest world deltas along that axis. The axes are the ENTITY's local
//! axes (its rotation applied) — dragging X moves along the entity's own
//! X, the way game editors do.
//!
//! Rotation is one ring in the view plane: drag the angle around the
//! gizmo's center, the entity rotates around the CAMERA's forward axis.
//! Screen angle uses atan2 in egui's y-down space; the composed delta
//! matches the drag's visual direction (pinned by
//! [`rotate_delta`] tests).

use crate::game::camera::{Camera, Projection};
use crate::game::editor::pick::{project_point, Viewport};
use crate::var::{Quat, Vec3};

/// Which gizmo the viewport shows (the toolbar's W/E/R).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoMode {
    Translate,
    Rotate,
    Scale,
}

impl GizmoMode {
    /// The mode's name (the toolbar button).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            GizmoMode::Translate => "move",
            GizmoMode::Rotate => "rotate",
            GizmoMode::Scale => "scale",
        }
    }

    /// Cycle W → E → R → W.
    #[must_use]
    pub fn next(self) -> GizmoMode {
        match self {
            GizmoMode::Translate => GizmoMode::Rotate,
            GizmoMode::Rotate => GizmoMode::Scale,
            GizmoMode::Scale => GizmoMode::Translate,
        }
    }
}

/// The gizmo's screen size: handle length from the center, in pixels.
pub const GIZMO_LEN_PX: f32 = 90.0;
/// The grab radius around any handle (generous on purpose).
pub const GRAB_PX: f32 = 12.0;

/// One axis's screen-space handle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisHandle {
    /// 0 = x, 1 = y, 2 = z (the entity's LOCAL axes).
    pub axis: usize,
    /// The axis's world direction (unit).
    pub world_dir: Vec3,
    /// The handle's far end in viewport pixels.
    pub screen_end: [f32; 2],
    /// The axis's screen direction (unit, from the center).
    pub screen_dir: [f32; 2],
    /// How many pixels one world unit covers ALONG this axis at the
    /// entity's depth (drag deltas divide by this).
    pub px_per_unit: f32,
    /// The axis is nearly parallel to the view direction — its screen
    /// footprint collapses; editors shrink it and so do we (still
    /// grabbable along whatever direction survives).
    pub foreshortened: bool,
}

/// The laid-out gizmo for one frame: everything the painter and the
/// hit-test need.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GizmoLayout {
    /// The entity's position in viewport pixels.
    pub center: [f32; 2],
    pub mode: GizmoMode,
    pub axes: [AxisHandle; 3],
    /// The rotation ring's radius (the Rotate mode only).
    pub ring_radius: f32,
}

/// The camera's view-plane basis in world space: (forward, right, up),
/// all unit — `right` matches the SCREEN's right (a step along it
/// projects toward larger pixel x; `up` toward smaller pixel y). The
/// cross order is `right = forward × world_up`, `up = right × forward` —
/// pinned by tests/editor.rs (`view_basis_right_matches_the_screen`)
/// after a would-be "fix" flipped it and broke every view-plane drag.
/// The ortho camera's sight is its -z (GL-style), so its view forward is
/// +z with the plain x/y basis.
#[must_use]
pub fn view_basis(camera: &Camera) -> (Vec3, Vec3, Vec3) {
    match camera.projection {
        Projection::Orthographic { .. } => (
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        ),
        Projection::Perspective { .. } => {
            let forward = camera.target - camera.position;
            let forward = if forward.length() > 0.0 {
                forward.normalize()
            } else {
                Vec3::new(0.0, 0.0, 1.0)
            };
            let world_up = Vec3::new(0.0, 1.0, 0.0);
            let right = forward.cross(world_up);
            let right = if right.length() > 0.0 {
                right.normalize()
            } else {
                Vec3::new(1.0, 0.0, 0.0)
            };
            let up = right.cross(forward).normalize();
            (forward, right, up)
        }
    }
}

/// px per world unit at `world`, measured along `dir` (a unit world
/// vector) — the drag-delta conversion factor. Measured by projecting a
/// small step: honest to whatever the projection does at that depth.
#[must_use]
fn px_per_unit(camera: &Camera, viewport: &Viewport, world: Vec3, dir: Vec3) -> f32 {
    let step = 0.1; // world units; small enough to be locally linear
    let Some(a) = project_point(camera, viewport, world) else {
        return 0.0;
    };
    let Some(b) = project_point(camera, viewport, world + dir.scaled(Vec3::splat(step))) else {
        return 0.0;
    };
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    (dx * dx + dy * dy).sqrt() / step
}

/// Lay out the gizmo for the selected entity (world position + rotation)
/// in the viewport. `None` when the entity is behind the camera.
#[must_use]
pub fn layout(
    mode: GizmoMode,
    camera: &Camera,
    viewport: &Viewport,
    position: Vec3,
    rotation: Quat,
) -> Option<GizmoLayout> {
    let center = project_point(camera, viewport, position)?;
    let local_axes = [
        (0, Vec3::new(1.0, 0.0, 0.0), [1.0, 0.35, 0.35]),
        (1, Vec3::new(0.0, 1.0, 0.0), [0.35, 1.0, 0.35]),
        (2, Vec3::new(0.0, 0.0, 1.0), [0.35, 0.55, 1.0]),
    ];
    let mut axes = [None, None, None];
    for (i, (axis, local, _color)) in local_axes.iter().enumerate() {
        let world_dir = rotation.rotate(*local);
        let px_per = px_per_unit(camera, viewport, position, world_dir);
        // the axis's on-screen direction: project a small step along it
        // (the projected motion is the drag mapping, whatever it is)
        let end_world = position + world_dir.scaled(Vec3::splat(0.25));
        let projected = project_point(camera, viewport, end_world);
        let mut screen_dir = [1.0, 0.0];
        if let Some(p) = projected {
            let d = [p[0] - center[0], p[1] - center[1]];
            let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
            if len > 1e-3 {
                screen_dir = [d[0] / len, d[1] / len];
            }
        }
        // nearly parallel to the view direction: the axis collapses on
        // screen; shrink it (overlap relief — a full-length axis pointing
        // at the camera reads as a blob over its siblings) and keep it
        // grabbable along whatever direction survives
        let foreshortened = px_per < 1.0;
        let len = if foreshortened { GIZMO_LEN_PX * 0.6 } else { GIZMO_LEN_PX };
        axes[i] = Some(AxisHandle {
            axis: *axis,
            world_dir,
            screen_end: [
                center[0] + screen_dir[0] * len,
                center[1] + screen_dir[1] * len,
            ],
            screen_dir,
            px_per_unit: px_per.max(0.05),
            foreshortened,
        });
    }
    let [x, y, z] = axes.map(|a| a.expect("three axes built"));
    Some(GizmoLayout {
        center,
        mode,
        axes: [x, y, z],
        ring_radius: GIZMO_LEN_PX * 0.6,
    })
}

/// What a pointer press landed on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GizmoHit {
    Axis(usize),
    Center,
    Ring,
    None,
}

/// Hit-test a pointer position against the laid-out gizmo. Axes win over
/// the ring; the center wins over everything (it is the smallest target
/// and the most intentional).
#[must_use]
pub fn hit_test(layout: &GizmoLayout, pointer: [f32; 2]) -> GizmoHit {
    let d = [
        pointer[0] - layout.center[0],
        pointer[1] - layout.center[1],
    ];
    let dist = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if dist <= GRAB_PX {
        return GizmoHit::Center;
    }
    // axes: distance from the pointer to each SEGMENT (center → end),
    // nearest wins — with an overlap TIE-BREAK: when two axes cover the
    // pointer nearly equally (a perspective view stacks Y over Z), the
    // axis more perpendicular to the view wins (larger px_per_unit) —
    // that one's drag is the meaningful motion, the other mostly points
    // at the camera
    let mut best_axis = None;
    let mut best_dist = GRAB_PX;
    let mut best_px_per = 0.0f32;
    for handle in &layout.axes {
        let end = handle.screen_end;
        let seg = [end[0] - layout.center[0], end[1] - layout.center[1]];
        let seg_len2 = seg[0] * seg[0] + seg[1] * seg[1];
        if seg_len2 < 1e-6 {
            continue;
        }
        let t = ((d[0] * seg[0] + d[1] * seg[1]) / seg_len2).clamp(0.0, 1.0);
        let px = layout.center[0] + seg[0] * t;
        let py = layout.center[1] + seg[1] * t;
        let dx = pointer[0] - px;
        let dy = pointer[1] - py;
        let distance = (dx * dx + dy * dy).sqrt();
        if distance > GRAB_PX {
            continue;
        }
        let takes = best_axis.is_none()
            || distance < best_dist - 2.0
            || (distance <= best_dist + 2.0 && handle.px_per_unit > best_px_per);
        if takes {
            best_dist = distance.min(best_dist);
            best_px_per = handle.px_per_unit;
            best_axis = Some(handle.axis);
        }
    }
    if let Some(axis) = best_axis {
        return GizmoHit::Axis(axis);
    }
    if layout.mode == GizmoMode::Rotate && (dist - layout.ring_radius).abs() <= GRAB_PX {
        return GizmoHit::Ring;
    }
    GizmoHit::None
}

/// A translate drag along one axis: screen delta → world position delta.
/// `axis` is the entity-local unit axis; `handle` the laid-out handle.
#[must_use]
pub fn translate_delta(handle: &AxisHandle, drag_px: [f32; 2]) -> Vec3 {
    let along = drag_px[0] * handle.screen_dir[0] + drag_px[1] * handle.screen_dir[1];
    let units = along / handle.px_per_unit.max(1e-6);
    handle.world_dir.scaled(Vec3::splat(units))
}

/// A translate drag on the CENTER square: move in the view plane
/// (camera right/up), the natural "just move it toward my cursor" mode.
#[must_use]
pub fn translate_center_delta(
    camera: &Camera,
    viewport: &Viewport,
    at: Vec3,
    drag_px: [f32; 2],
) -> Vec3 {
    let (_, right, up) = view_basis(camera);
    let f_right = px_per_unit(camera, viewport, at, right).max(1e-6);
    let f_up = px_per_unit(camera, viewport, at, up).max(1e-6);
    right.scaled(Vec3::splat(drag_px[0] / f_right))
        + up.scaled(Vec3::splat(-drag_px[1] / f_up))
}

/// A rotate drag: the screen angle (atan2, y-down) swept around the
/// center since the drag started, and the camera's forward axis. Returns
/// the quaternion delta to LEFT-MULTIPLY onto the entity's rotation
/// (`new = delta.compose(current)` — a rotation in VIEW space, the way
/// editors turn things toward your hand).
#[must_use]
pub fn rotate_delta(angle_delta: f32, camera_forward: Vec3) -> Quat {
    Quat::axis_angle(camera_forward, angle_delta)
}

/// A scale drag along one axis: screen delta → the world-unit delta to
/// ADD to that axis's scale component (the caller clamps above zero).
/// Scale is additive under drag: predictable, and small scales stay
/// usable.
#[must_use]
pub fn scale_delta(handle: &AxisHandle, drag_px: [f32; 2]) -> f32 {
    let along = drag_px[0] * handle.screen_dir[0] + drag_px[1] * handle.screen_dir[1];
    along / handle.px_per_unit.max(1e-6)
}
