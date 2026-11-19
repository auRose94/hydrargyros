//! The Studio editor core ([game-editor-design.md](../../../docs/game-editor-design.md)
//! M1): an egui shell around a live [`Engine`](crate::game::Engine) — the
//! editor IS the engine running with panels on top.
//!
//! Two halves, split for the headless-first rule:
//!
//! - [`ui`] — the editor's STATE and OPERATIONS (selection, hierarchy
//!   snapshots, inspector edits), pure data over the engine's public
//!   surface. No egui, no wgpu, no window: it runs and tests headless.
//! - [`shell`] (behind the `editor` feature) — the window: a winit event
//!   loop, the engine pump on its own thread, the engine's frame blitted
//!   under egui panels on one surface.

pub mod camera_ctl;
pub mod debug;
pub mod gizmo;
pub mod pick;
pub mod ui;
#[cfg(feature = "editor")]
pub mod shell;
#[cfg(feature = "editor")]
pub mod style;
