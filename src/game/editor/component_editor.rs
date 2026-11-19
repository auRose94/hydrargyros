//! Component-editor extensions: components teach the inspector how to
//! edit THEM. A component registers an editor function under its
//! document type name (`scene_type`); when that component is on the
//! selected entity, the inspector hands it egui access — sliders,
//! toggles, and buttons for in-editor preprocessing (a test bake, a
//! conversion) — instead of the generic parameter summary.
//!
//! The registry is session-global (the function-table pattern): game
//! code registers at startup or on attach; the editor looks up by
//! `scene_type` per frame. Nothing here is stored in documents — an
//! editor is TOOLING, the scene data stays the component's params.
//!
//! Editors MUTATE their component live (the paused editor re-renders
//! the viewport after every edit through the scene-dirty gate), and
//! may return an [`EditorAction`] — a deferred engine mutation (a
//! button's preprocess step) applied by the inspector after the paint,
//! so the component borrow never overlaps the engine's.

use crate::game::component::Component;
use crate::game::Engine;
use egui::Ui;
use std::any::TypeId;
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// What a component's editor asks the shell to DO after the paint —
/// the deferred half of an edit. `None` = the paint was the whole
/// story (a slider's write already landed on the component).
pub enum EditorAction {
    /// Nothing further.
    None,
    /// Run a preprocess step against the engine (a "bake normals"
    /// button's work, a "convert geometry" pass).
    Engine(Box<dyn FnOnce(&mut Engine)>),
}

impl EditorAction {
    /// Apply the deferred work (the inspector calls this after the
    /// component borrow ends).
    pub fn run(self, engine: &mut Engine) {
        match self {
            EditorAction::None => {}
            EditorAction::Engine(run) => run(engine),
        }
    }
}

/// A component's inspector extension: egui access, a mutable view of
/// the component, and the ORIGIN entity's index path (for actions that
/// need to know where they are). Pure paint + mutation — deferred
/// engine work goes through the returned [`EditorAction`].
pub type EditorFn = fn(&mut Ui, &mut dyn Component, origin: &[usize]) -> Option<EditorAction>;

fn registry() -> &'static RwLock<HashMap<TypeId, EditorFn>> {
    static REGISTRY: OnceLock<RwLock<HashMap<TypeId, EditorFn>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register an editor for a COMPONENT TYPE (the Rust type — no
/// document-name requirement, so runtime-only components mod their own
/// inspectors too). Re-registering replaces — the registry pattern
/// every hyg backend follows.
pub fn register_component_editor<T: Component + 'static>(editor: EditorFn) {
    registry()
        .write()
        .unwrap()
        .insert(TypeId::of::<T>(), editor);
}

/// The registered editor for a component's TypeId, if any. Components
/// without one fall back to the inspector's generic summary.
#[must_use]
pub fn editor_for_id(type_id: TypeId) -> Option<EditorFn> {
    registry().read().unwrap().get(&type_id).copied()
}

/// The registered editors' TypeIds (the debug view's introspection).
#[must_use]
pub fn registered_ids() -> Vec<TypeId> {
    let mut ids: Vec<TypeId> = registry().read().unwrap().keys().copied().collect();
    ids.sort_by_key(|id| format!("{id:?}"));
    ids
}

/// The convenience wrapper: the editor for a component instance.
#[must_use]
pub fn editor_for_component(component: &dyn Component) -> Option<EditorFn> {
    editor_for_id(component.as_any().type_id())
}
