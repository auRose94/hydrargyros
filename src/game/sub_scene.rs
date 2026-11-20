//! Sub-scene composition — the `sub_scene` component, hyg's prefab
//! shape. An entity carrying one is an *instance*: the scene file its
//! `path` names is loaded at load time and its entities splice in as
//! the instance entity's children, positioned by its transform.
//!
//! ```json
//! {
//!   "name": "room_3",
//!   "transform": { "position": [12, 0, -4] },
//!   "components": [{ "type": "sub_scene", "path": "rooms/room.json.hyg" }]
//! }
//! ```
//!
//! The reference is the truth — the prefab model:
//!
//! - **Instances regenerate.** The spliced children are live entities
//!   (selectable, positioned by the instance entity), but the save and
//!   snapshot walks skip them — the document carries only the
//!   `sub_scene` reference, and every load re-splices from the file.
//!   Edits made to spliced children are session edits, like play-mode
//!   edits; changing the prefab file changes every instance.
//! - **References resolve against the sub-scene's own directory** —
//!   the same rule every file-referencing param follows. A prefab in
//!   `rooms/` names its scripts and nested sub-scenes relative to
//!   `rooms/`, wherever the instancing scene lives.
//! - **Cycles refuse loudly.** A scene including itself (directly or
//!   through a chain) is an explicit load error naming the cycle —
//!   never a hang.
//!
//! The sub-document's own `camera` is ignored: instances contribute
//! entities only, the instancing scene keeps its view. Bundles carry
//! sub-scenes and their dependencies automatically (the bundler walks
//! the same references the loader does).

use crate::game::component::Component;
use crate::game::scene::{req_string, ComponentContext};
use crate::o;
use crate::var::Var;
use crate::FromVarError;

/// A sub-scene instance: the entity carries this, the named scene's
/// entities splice in as its children at load time.
#[derive(Debug, Clone, PartialEq)]
pub struct SubScene {
    /// The scene file's path, relative to the scene file this component
    /// was loaded from (kept verbatim — the document round-trips it).
    path: String,
}

impl SubScene {
    /// An instance of the scene at `path` (relative to the containing
    /// scene file).
    pub fn new(path: impl Into<String>) -> SubScene {
        SubScene { path: path.into() }
    }

    /// The referenced scene file's path, as the document wrote it.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Repoint the instance (the editor inspector's edit row). The new
    /// scene splices on the next load of the scene.
    pub fn set_path(&mut self, path: impl Into<String>) {
        self.path = path.into();
    }

    /// Parse the document params (`{ "path": … }` — the path is
    /// required and non-empty). No file IO here: the splice happens at
    /// the scene walk's level, after the component materializes.
    pub(crate) fn from_params(params: &Var) -> Result<SubScene, FromVarError> {
        Ok(SubScene {
            path: req_string(params, "path")?,
        })
    }
}

impl Component for SubScene {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn scene_type(&self) -> Option<&'static str> {
        Some("sub_scene")
    }

    fn scene_params(&self, _ctx: &ComponentContext) -> Option<Var> {
        Some(Var::from(o! { "path": Var::from(self.path.clone()) }))
    }
}
