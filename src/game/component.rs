//! Components — gold's `component`: priority-tiered behavior with an
//! initialize / update / draw lifecycle.

use std::any::Any;

/// When a component's `update` runs in the frame. gold's priority tiers:
/// things that must run first (input), things that load data, physics
/// bodies, user space, and renderables — which run last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// Tier 0 — input and other must-run-first behavior.
    High,
    /// Tier 10 — things that need loading.
    Data,
    /// Tier 100 — physics bodies and shaped loading.
    Physics,
    /// Tier 1000 — user space (gold allows 1000 → 1_000_000).
    Generic,
    /// Tier 1_000_000 — reserved for renderables; called last.
    Draw,
    /// A custom tier in user space.
    Custom(i64),
}

impl Priority {
    /// The numeric tier used for sorting (lower runs earlier).
    #[must_use]
    pub fn value(self) -> i64 {
        match self {
            Priority::High => 0,
            Priority::Data => 10,
            Priority::Physics => 100,
            Priority::Generic => 1000,
            Priority::Draw => 1_000_000,
            Priority::Custom(v) => v,
        }
    }
}

/// A behavior attached to an [`Entity`](crate::game::Entity) — gold's
/// `component`, as a trait. Components are native Rust objects (the hot
/// path); scripted components arrive later with the interpreter seam.
pub trait Component: Send {
    /// Which tier of the frame this component updates in.
    fn priority(&self) -> Priority {
        Priority::Generic
    }

    /// Called once, the first frame the component exists in the engine.
    fn initialize(&mut self) {}

    /// Called every frame, in priority order, with the frame's delta time
    /// in seconds.
    fn update(&mut self, _dt: f64) {}

    /// Called in the draw pass (after all updates), in priority order.
    fn draw(&mut self) {}

    /// Typed lookup support (`entity.get_components::<T>()`).
    fn as_any(&self) -> &dyn Any;

    /// Mutable typed lookup — the engine's physics pass configures
    /// components that way (materializing bodies).
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// If this component contributes geometry to the drawn scene (gold's
    /// renderables), the engine's draw pass picks it up here.
    fn as_renderable(&self) -> Option<&dyn super::renderable::Renderable> {
        None
    }

    /// If this component is a light (gold's `light` component), the
    /// engine's draw pass gathers it — with its entity's world transform —
    /// into the scene's light list.
    fn as_light(&self) -> Option<&super::light::Light> {
        None
    }
}
