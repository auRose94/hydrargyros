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

/// One reversible edit: the old and new of one field at one entity path
/// (the design's `(path, old, new)` put-record). A transform drag
/// coalesces into ONE command — see [`UndoStack::push_coalescing`].
///
/// A [`Command::Delete`] OWNS the removed subtree: it sits on the undo
/// stack with the entity held (`Some`) — the engine has already dropped
/// it — and on the redo stack empty (`None`) after the undo re-inserted
/// it. Undo/redo move the entity between the engine and the command.
pub enum Command {
    Position {
        path: Vec<usize>,
        old: Vec3,
        new: Vec3,
    },
    Scale {
        path: Vec<usize>,
        old: Vec3,
        new: Vec3,
    },
    Rotation {
        path: Vec<usize>,
        old: Quat,
        new: Quat,
    },
    Rename {
        path: Vec<usize>,
        old: String,
        new: String,
    },
    Enabled {
        path: Vec<usize>,
        old: bool,
        new: bool,
    },
    Delete {
        path: Vec<usize>,
        /// `Some` while the subtree is out of the engine (undoable);
        /// `None` while it is back in the engine (redoable).
        entity: Option<Box<Entity>>,
    },
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Entity has no Debug (dyn components); the subtree prints as a
        // placeholder
        match self {
            Command::Position { path, old, new } => f
                .debug_struct("Position")
                .field("path", path)
                .field("old", old)
                .field("new", new)
                .finish(),
            Command::Scale { path, old, new } => f
                .debug_struct("Scale")
                .field("path", path)
                .field("old", old)
                .field("new", new)
                .finish(),
            Command::Rotation { path, old, new } => f
                .debug_struct("Rotation")
                .field("path", path)
                .field("old", old)
                .field("new", new)
                .finish(),
            Command::Rename { path, old, new } => f
                .debug_struct("Rename")
                .field("path", path)
                .field("old", old)
                .field("new", new)
                .finish(),
            Command::Enabled { path, old, new } => f
                .debug_struct("Enabled")
                .field("path", path)
                .field("old", old)
                .field("new", new)
                .finish(),
            Command::Delete { path, entity } => f
                .debug_struct("Delete")
                .field("path", path)
                .field("entity", &entity.is_some())
                .finish(),
        }
    }
}

impl Command {
    /// The path the command touched (the coalescing key's prefix and the
    /// UI's "undo targets this row" signal).
    #[must_use]
    pub fn path(&self) -> &[usize] {
        match self {
            Command::Position { path, .. }
            | Command::Scale { path, .. }
            | Command::Rotation { path, .. }
            | Command::Rename { path, .. }
            | Command::Enabled { path, .. }
            | Command::Delete { path, .. } => path,
        }
    }

    /// Make the engine look like it did AFTER the edit (the push and the
    /// redo). A Delete's redo re-removes the subtree and re-stashes it so
    /// the command stays undoable.
    fn apply(&mut self, engine: &mut Engine) {
        match self {
            Command::Position { path, new, .. } => {
                if let Some(entity) = engine.entity_at_mut(path) {
                    entity.transform_mut().position = *new;
                }
            }
            Command::Scale { path, new, .. } => {
                if let Some(entity) = engine.entity_at_mut(path) {
                    entity.transform_mut().scale = *new;
                }
            }
            Command::Rotation { path, new, .. } => {
                if let Some(entity) = engine.entity_at_mut(path) {
                    entity.transform_mut().rotation = *new;
                }
            }
            Command::Rename { path, new, .. } => {
                if let Some(entity) = engine.entity_at_mut(path) {
                    entity.set_name(new.clone());
                }
            }
            Command::Enabled { path, new, .. } => {
                if let Some(entity) = engine.entity_at_mut(path) {
                    entity.set_enabled(*new);
                }
            }
            Command::Delete { path, entity } => {
                if entity.is_none() {
                    *entity = engine.remove_entity(path).map(Box::new);
                }
            }
        }
    }

    /// Make the engine look like it did BEFORE the edit (the undo). A
    /// Delete's undo CONSUMES the held subtree — the command lands on the
    /// redo stack empty (its redo re-removes).
    fn unapply(&mut self, engine: &mut Engine) {
        match self {
            Command::Position { path, old, .. } => {
                if let Some(entity) = engine.entity_at_mut(path) {
                    entity.transform_mut().position = *old;
                }
            }
            Command::Scale { path, old, .. } => {
                if let Some(entity) = engine.entity_at_mut(path) {
                    entity.transform_mut().scale = *old;
                }
            }
            Command::Rotation { path, old, .. } => {
                if let Some(entity) = engine.entity_at_mut(path) {
                    entity.transform_mut().rotation = *old;
                }
            }
            Command::Rename { path, old, .. } => {
                if let Some(entity) = engine.entity_at_mut(path) {
                    entity.set_name(old.clone());
                }
            }
            Command::Enabled { path, old, .. } => {
                if let Some(entity) = engine.entity_at_mut(path) {
                    entity.set_enabled(*old);
                }
            }
            Command::Delete { path, entity } => {
                if let Some(subtree) = entity.take() {
                    engine.insert_entity(path, *subtree);
                }
            }
        }
    }
}

/// The editor's undo/redo stacks. `push_coalescing` folds a new command
/// into the last when both touch the SAME field of the SAME entity within
/// the coalescing window — a continuous drag becomes one undo step, not
/// one per frame.
#[derive(Debug, Default)]
pub struct UndoStack {
    undo: Vec<Command>,
    redo: Vec<Command>,
    /// Millis timestamp of the last push (coalescing window bookkeeping).
    last_push_ms: u64,
}

/// The coalescing window: edits of the same field within this many
/// milliseconds fold into one command. A drag's per-frame edits land
/// milliseconds apart; two deliberate clicks are seconds apart.
const COALESCE_WINDOW_MS: u64 = 400;

impl UndoStack {
    pub fn new() -> UndoStack {
        UndoStack::default()
    }

    /// Push a completed command (redo stack cleared — standard linear
    /// history).
    pub fn push(&mut self, command: Command, now_ms: u64) {
        self.undo.push(command);
        self.redo.clear();
        self.last_push_ms = now_ms;
    }

    /// Push a command, folding it into the last when it continues the
    /// same field of the same entity inside the window (a drag's
    /// per-frame writes). The folded command KEEPS the last's OLD value —
    /// one undo returns to where the gesture started.
    pub fn push_coalescing(&mut self, command: Command, now_ms: u64) {
        // outside the window (or nothing to fold into): a fresh step
        if now_ms.saturating_sub(self.last_push_ms) > COALESCE_WINDOW_MS || self.undo.is_empty() {
            self.push(command, now_ms);
            return;
        }
        // same field (variant) and same entity: the last command's NEW
        // value becomes this command's NEW value — one undo per drag
        let coalesce = match (self.undo.last(), &command) {
            (
                Some(Command::Position { path: p, .. }),
                Command::Position { path, .. },
            ) => p == path,
            (
                Some(Command::Scale { path: p, .. }),
                Command::Scale { path, .. },
            ) => p == path,
            (
                Some(Command::Rotation { path: p, .. }),
                Command::Rotation { path, .. },
            ) => p == path,
            _ => false,
        };
        if !coalesce {
            self.push(command, now_ms);
            return;
        }
        match (self.undo.last_mut(), command) {
            (
                Some(Command::Position { new: last_new, .. }),
                Command::Position { new, .. },
            ) => *last_new = new,
            (Some(Command::Scale { new: last_new, .. }), Command::Scale { new, .. }) => {
                *last_new = new;
            }
            (Some(Command::Rotation { new: last_new, .. }), Command::Rotation { new, .. }) => {
                *last_new = new;
            }
            _ => unreachable!("coalesce checked the variant and path above"),
        }
        self.last_push_ms = now_ms;
    }

    /// Undo the newest command. `false` when the stack is empty.
    pub fn undo(&mut self, engine: &mut Engine) -> bool {
        let Some(mut command) = self.undo.pop() else {
            return false;
        };
        command.unapply(engine);
        self.redo.push(command);
        true
    }

    /// Redo the most recently undone command. `false` when empty.
    pub fn redo(&mut self, engine: &mut Engine) -> bool {
        let Some(mut command) = self.redo.pop() else {
            return false;
        };
        command.apply(engine);
        self.undo.push(command);
        true
    }

    /// Anything to undo / redo (the toolbar buttons' enabled state).
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Clear both stacks (a scene load invalidates every stored path).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// One history row: where the timeline sits after this command, its
    /// label, and whether it is currently applied (`done`). The redo
    /// branch's rows are `done: false` — clicking any row jumps there.
    #[must_use]
    pub fn history(&self, engine: &Engine) -> Vec<HistoryEntry> {
        let mut entries: Vec<HistoryEntry> = self
            .undo
            .iter()
            .enumerate()
            .map(|(i, command)| HistoryEntry {
                timeline_index: i + 1,
                label: command.describe(engine),
                done: true,
            })
            .collect();
        let undone_at = entries.len();
        for (i, command) in self.redo.iter().rev().enumerate() {
            entries.push(HistoryEntry {
                timeline_index: undone_at + i + 1,
                label: command.describe(engine),
                done: false,
            });
        }
        entries
    }

    /// The timeline cursor (0 = the state before any recorded command;
    /// `undo.len()` = now). The history panel's highlight.
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.undo.len()
    }

    /// Jump the timeline to `index` — undoing/redoing as many commands as
    /// it takes (Godot's click-a-row-to-travel). `true` when anything
    /// moved.
    pub fn goto(&mut self, index: usize, engine: &mut Engine) -> bool {
        let start = self.undo.len();
        while self.undo.len() > index {
            if !self.undo(engine) {
                break;
            }
        }
        while self.undo.len() < index {
            if !self.redo(engine) {
                break;
            }
        }
        self.undo.len() != start
    }
}

/// One row of the history panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    /// The timeline position AFTER this command (`goto` target).
    pub timeline_index: usize,
    pub label: String,
    /// Applied (on the undo stack) vs un-done (on the redo branch).
    pub done: bool,
}

impl Command {
    /// The panel label: verb + entity name (resolved live; a stale path
    /// degrades to the held subtree's name for Deletes, else "entity" —
    /// the row still travels correctly either way).
    #[must_use]
    pub fn describe(&self, engine: &Engine) -> String {
        let name = match self {
            // the deleted subtree is out of the engine — its name rides
            // in the command
            Command::Delete {
                entity: Some(held), ..
            } => held.name().to_string(),
            _ => engine
                .entity_at(self.path())
                .map(|e| e.name().to_string())
                .unwrap_or_else(|| "entity".to_string()),
        };
        match self {
            Command::Position { .. } => format!("move {name}"),
            Command::Scale { .. } => format!("scale {name}"),
            Command::Rotation { .. } => format!("rotate {name}"),
            Command::Rename { new, .. } => format!("rename → {new}"),
            Command::Enabled { new, .. } => {
                if *new { format!("enable {name}") } else { format!("disable {name}") }
            }
            Command::Delete { .. } => format!("delete {name}"),
        }
    }
}

/// The editor's state: what is selected, and everything the panels need
/// to remember between frames. All edits go through the undo stack — a
/// drag coalesces into one step (see [`UndoStack::push_coalescing`]).
#[derive(Debug, Default)]
pub struct EditorUi {
    /// The selected entity's tree path, if any.
    selected: Option<Vec<usize>>,
    undo: UndoStack,
}

/// Millis for the undo coalescing window — the shell passes a monotonic
/// clock; tests pass synthetic stamps.
pub type NowMs = u64;

impl EditorUi {
    /// An editor with nothing selected.
    pub fn new() -> EditorUi {
        EditorUi {
            selected: None,
            undo: UndoStack::new(),
        }
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

    /// The undo stack (the toolbar's enabled states).
    #[must_use]
    pub fn undo_stack(&self) -> &UndoStack {
        &self.undo
    }

    /// The undo stack, mutable — the history panel's timeline travel.
    pub fn undo_stack_mut(&mut self) -> &mut UndoStack {
        &mut self.undo
    }

    /// Undo the newest edit. `false` when nothing to undo.
    pub fn undo(&mut self, engine: &mut Engine) -> bool {
        self.undo.undo(engine)
    }

    /// Redo the most recently undone edit. `false` when nothing to redo.
    pub fn redo(&mut self, engine: &mut Engine) -> bool {
        self.undo.redo(engine)
    }

    /// Delete the selected entity (its subtree is kept for undo).
    /// `false` when nothing live is selected. Deselects — the path no
    /// longer exists.
    pub fn delete_selected(&mut self, engine: &mut Engine, now_ms: NowMs) -> bool {
        let Some(path) = self.selected.as_deref().map(|p| p.to_vec()) else {
            return false;
        };
        let Some(entity) = engine.remove_entity(&path) else {
            return false;
        };
        self.undo.push(
            Command::Delete {
                path,
                entity: Some(Box::new(entity)),
            },
            now_ms,
        );
        self.deselect();
        true
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

    // ---- writes (the inspector's edits; each records an undo command) ----

    /// Rename the selected entity. `false` when nothing (or nothing
    /// live) is selected.
    pub fn rename_selected(&mut self, engine: &mut Engine, name: &str, now_ms: NowMs) -> bool {
        let Some(path) = self.selected.as_deref().map(|p| p.to_vec()) else {
            return false;
        };
        let Some(entity) = engine.entity_at(&path) else {
            return false;
        };
        let old = entity.name().to_string();
        engine.entity_at_mut(&path).unwrap().set_name(name);
        self.undo.push_coalescing(
            Command::Rename {
                path,
                old,
                new: name.to_string(),
            },
            now_ms,
        );
        true
    }

    /// Toggle the selected entity's enabled flag. `false` when nothing
    /// (or nothing live) is selected.
    pub fn set_selected_enabled(
        &mut self,
        engine: &mut Engine,
        enabled: bool,
        now_ms: NowMs,
    ) -> bool {
        let Some(path) = self.selected.as_deref().map(|p| p.to_vec()) else {
            return false;
        };
        let Some(entity) = engine.entity_at(&path) else {
            return false;
        };
        let old = entity.enabled();
        engine
            .entity_at_mut(&path)
            .unwrap()
            .set_enabled(enabled);
        self.undo.push_coalescing(Command::Enabled { path, old, new: enabled }, now_ms);
        true
    }

    /// Set the selected entity's position. `false` when nothing (or
    /// nothing live) is selected.
    pub fn set_selected_position(
        &mut self,
        engine: &mut Engine,
        position: Vec3,
        now_ms: NowMs,
    ) -> bool {
        let Some(path) = self.selected.as_deref().map(|p| p.to_vec()) else {
            return false;
        };
        let Some(entity) = engine.entity_at(&path) else {
            return false;
        };
        let old = entity.transform().position;
        engine.entity_at_mut(&path).unwrap().transform_mut().position = position;
        self.undo.push_coalescing(
            Command::Position {
                path,
                old,
                new: position,
            },
            now_ms,
        );
        true
    }

    /// Set the selected entity's rotation. `false` when nothing (or
    /// nothing live) is selected.
    pub fn set_selected_rotation(
        &mut self,
        engine: &mut Engine,
        rotation: Quat,
        now_ms: NowMs,
    ) -> bool {
        let Some(path) = self.selected.as_deref().map(|p| p.to_vec()) else {
            return false;
        };
        let Some(entity) = engine.entity_at(&path) else {
            return false;
        };
        let old = entity.transform().rotation;
        engine.entity_at_mut(&path).unwrap().transform_mut().rotation = rotation;
        self.undo.push_coalescing(
            Command::Rotation {
                path,
                old,
                new: rotation,
            },
            now_ms,
        );
        true
    }

    /// Set the selected entity's scale. `false` when nothing (or nothing
    /// live) is selected.
    pub fn set_selected_scale(&mut self, engine: &mut Engine, scale: Vec3, now_ms: NowMs) -> bool {
        let Some(path) = self.selected.as_deref().map(|p| p.to_vec()) else {
            return false;
        };
        let Some(entity) = engine.entity_at(&path) else {
            return false;
        };
        let old = entity.transform().scale;
        engine.entity_at_mut(&path).unwrap().transform_mut().scale = scale;
        self.undo.push_coalescing(Command::Scale { path, old, new: scale }, now_ms);
        true
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
