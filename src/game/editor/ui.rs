//! The editor's state and operations — the headless-testable half of the
//! Studio core. No egui, no wgpu, no window: everything here is plain
//! data over the engine's public surface, so the UI logic runs and tests
//! without a display. The shell's panels are a thin view over this: they
//! read the snapshots and call the operations.
//!
//! Selection addressing is an **entity path** — `[root_idx, child_idx,
//! …]` walking [`Entity::children`](crate::game::Entity::children) —
//! resolved through
//! [`Engine::entity_at`](crate::game::Engine::entity_at). Indices move
//! when the tree restructures (M2's reparenting); that is a known v1
//! limit, not a forever contract.

use crate::game::engine::Engine;
use crate::game::entity::Entity;
use crate::var::{Quat, Vec3};

/// The editor's state: what is selected, and everything the panels need
/// to remember between frames. Grows with M2 (undo stack, gizmo mode).
#[derive(Debug, Default)]
pub struct EditorUi {
    /// The selected entity's tree path, if any.
    selected: Option<Vec<usize>>,
}

impl EditorUi {
    /// An editor with nothing selected.
    pub fn new() -> EditorUi {
        EditorUi { selected: None }
    }

    /// Select the entity at a tree path.
    pub fn select(&mut self, path: Vec<usize>) {
        self.selected = Some(path);
    }

    /// Clear the selection (a click on empty space).
    pub fn deselect(&mut self) {
        self.selected = None;
    }

    /// The selected entity's tree path, if any.
    #[must_use]
    pub fn selection(&self) -> Option<&[usize]> {
        self.selected.as_deref()
    }

    // ---- reads (the panels' data) ----

    /// The whole tree as snapshot nodes — the hierarchy panel's rows,
    /// each carrying the path that selects it. A pure read: the engine is
    /// untouched.
    #[must_use]
    pub fn hierarchy(engine: &Engine) -> Vec<HierarchyNode> {
        engine
            .entities()
            .iter()
            .enumerate()
            .map(|(i, e)| hierarchy_of(e, &[i]))
            .collect()
    }

    /// The selected entity, if it still exists (indices may have moved
    /// since the click — the panel re-checks every frame).
    #[must_use]
    pub fn selected_entity<'a>(&self, engine: &'a Engine) -> Option<&'a Entity> {
        engine.entity_at(self.selected.as_deref()?)
    }

    // ---- writes (the inspector's edits) ----

    /// Rename the selected entity. `false` when nothing (or nothing
    /// live) is selected.
    pub fn rename_selected(&self, engine: &mut Engine, name: &str) -> bool {
        match engine.entity_at_mut(self.selected.as_deref().unwrap_or(&[])) {
            Some(entity) => {
                entity.set_name(name);
                true
            }
            None => false,
        }
    }

    /// Toggle the selected entity's enabled flag. `false` when nothing
    /// (or nothing live) is selected.
    pub fn set_selected_enabled(&self, engine: &mut Engine, enabled: bool) -> bool {
        match engine.entity_at_mut(self.selected.as_deref().unwrap_or(&[])) {
            Some(entity) => {
                entity.set_enabled(enabled);
                true
            }
            None => false,
        }
    }

    /// Set the selected entity's position. `false` when nothing (or
    /// nothing live) is selected.
    pub fn set_selected_position(&self, engine: &mut Engine, position: Vec3) -> bool {
        match engine.entity_at_mut(self.selected.as_deref().unwrap_or(&[])) {
            Some(entity) => {
                entity.transform_mut().position = position;
                true
            }
            None => false,
        }
    }

    /// Set the selected entity's rotation. `false` when nothing (or
    /// nothing live) is selected.
    pub fn set_selected_rotation(&self, engine: &mut Engine, rotation: Quat) -> bool {
        match engine.entity_at_mut(self.selected.as_deref().unwrap_or(&[])) {
            Some(entity) => {
                entity.transform_mut().rotation = rotation;
                true
            }
            None => false,
        }
    }

    /// Set the selected entity's scale. `false` when nothing (or nothing
    /// live) is selected.
    pub fn set_selected_scale(&self, engine: &mut Engine, scale: Vec3) -> bool {
        match engine.entity_at_mut(self.selected.as_deref().unwrap_or(&[])) {
            Some(entity) => {
                entity.transform_mut().scale = scale;
                true
            }
            None => false,
        }
    }
}

/// One hierarchy row: an entity, its path, and its component type names.
#[derive(Debug, Clone, PartialEq)]
pub struct HierarchyNode {
    pub name: String,
    pub enabled: bool,
    /// The tree path selecting this entity back.
    pub path: Vec<usize>,
    /// The entity's components' scene type names, in attach order
    /// (runtime-only components included — the inspector shows what is
    /// there, the scene save's rules are its own).
    pub component_types: Vec<String>,
    pub children: Vec<HierarchyNode>,
}

fn hierarchy_of(entity: &Entity, path: &[usize]) -> HierarchyNode {
    HierarchyNode {
        name: entity.name().to_string(),
        enabled: entity.enabled(),
        path: path.to_vec(),
        component_types: entity
            .iter_components()
            .map(|c| c.scene_type().unwrap_or("(runtime)").to_string())
            .collect(),
        children: entity
            .children()
            .iter()
            .enumerate()
            .map(|(i, child)| {
                let child_path: Vec<usize> = path.iter().copied().chain([i]).collect();
                hierarchy_of(child, &child_path)
            })
            .collect(),
    }
}
