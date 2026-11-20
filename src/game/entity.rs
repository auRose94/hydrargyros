//! Entities — the `Entity` scene node: a transform, components, and child
//! entities, enable/disable included.

use super::component::Component;
use super::transform::Transform;

/// A component plus its engine bookkeeping (initialization state).
pub struct ComponentSlot {
    pub(crate) component: Box<dyn Component>,
    pub(crate) initialized: bool,
}

/// A node in the scene: transform, components, children.
/// Disabled entities skip their components entirely (and their subtrees).
pub struct Entity {
    name: String,
    transform: Transform,
    components: Vec<ComponentSlot>,
    children: Vec<Entity>,
    enabled: bool,
    /// Set on the children a parent's `sub_scene` instance spliced in:
    /// they regenerate from the referenced file at every load, so the
    /// save and snapshot walks skip them (the reference is the truth).
    spliced: bool,
}

impl Entity {
    pub fn new(name: impl Into<String>) -> Entity {
        Entity {
            name: name.into(),
            transform: Transform::default(),
            components: Vec::new(),
            children: Vec::new(),
            enabled: true,
            spliced: false,
        }
    }

    /// Whether this entity was spliced in by a parent's
    /// [`SubScene`](super::sub_scene::SubScene) instance. Spliced
    /// entities live in the tree (selectable, transformable) but are
    /// not written by the scene save — the sub_scene reference is what
    /// the document keeps.
    pub fn is_spliced(&self) -> bool {
        self.spliced
    }

    /// Mark this entity as a sub-scene instance's spliced content.
    pub(crate) fn set_spliced(&mut self) {
        self.spliced = true;
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Rename the entity (the editor inspector's name row). The
    /// script-facing handle — anything holding the old name sees the new
    /// one on its next lookup.
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    pub fn transform(&self) -> &Transform {
        &self.transform
    }

    pub fn transform_mut(&mut self) -> &mut Transform {
        &mut self.transform
    }

    /// Attach a component (appended; initialized on its first frame).
    pub fn add_component(&mut self, component: Box<dyn Component>) -> &mut Self {
        self.components.push(ComponentSlot {
            component,
            initialized: false,
        });
        self
    }

    /// Attach a child entity (appended to the children list).
    pub fn add_child(&mut self, child: Entity) -> &mut Self {
        self.children.push(child);
        self
    }

    /// Remove a child by index, returning it (the editor's delete).
    /// `None` out of range.
    pub(crate) fn remove_child(&mut self, idx: usize) -> Option<Entity> {
        (idx < self.children.len()).then(|| self.children.remove(idx))
    }

    /// Insert a child AT an index (clamped into range — the delete-undo's
    /// restore seam).
    pub(crate) fn insert_child(&mut self, idx: usize, child: Entity) {
        let idx = idx.min(self.children.len());
        self.children.insert(idx, child);
    }

    /// The entity's components, in attach order — the scene save walk and
    /// the editor's inspector read them through this.
    pub fn iter_components(&self) -> impl Iterator<Item = &dyn Component> + '_ {
        self.components.iter().map(|slot| slot.component.as_ref())
    }

    /// The components, MUTABLE — the editor's component-editor
    /// extensions write through this (sliders, toggles, preprocess
    /// buttons), the same walk the scene save reads.
    pub fn iter_components_mut<'a>(
        &'a mut self,
    ) -> impl Iterator<Item = &'a mut (dyn Component + 'static)> + 'a {
        self.components
            .iter_mut()
            .map(|slot| slot.component.as_mut())
    }

    pub fn children(&self) -> &[Entity] {
        &self.children
    }

    /// The children, mutable — the editor's live-reload walk.
    // the editor's reparent walk is the only mutator (gated — the
    // feature-free build never mutates children directly)
    #[cfg(feature = "editor")]
    pub(crate) fn children_mut(&mut self) -> &mut Vec<Entity> {
        &mut self.children
    }

    /// A mutable child by index (the engine's physics sync walks these).
    pub(crate) fn child_mut(&mut self, idx: usize) -> Option<&mut Entity> {
        self.children.get_mut(idx)
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Typed component lookup (by downcasting): the first component of
    /// type `T` on this entity.
    pub fn get_component<T: Component + 'static>(&self) -> Option<&T> {
        self.components
            .iter()
            .find_map(|slot| slot.component.as_any().downcast_ref::<T>())
    }

    /// The entity's component of type `T`, mutable — the inspector's
    /// camera controls write through this.
    pub fn component_mut<T: Component + 'static>(&mut self) -> Option<&mut T> {
        self.components
            .iter_mut()
            .find_map(|slot| slot.component.as_any_mut().downcast_mut::<T>())
    }

    /// All components of type `T`, including children's — recursive
    /// down the whole subtree.
    pub fn get_components_recursive<T: Component + 'static>(&self) -> Vec<&T> {
        let mut found: Vec<&T> = self
            .components
            .iter()
            .filter_map(|slot| slot.component.as_any().downcast_ref::<T>())
            .collect();
        for child in &self.children {
            found.extend(child.get_components_recursive::<T>());
        }
        found
    }

    /// The slot at a tree path (`[entity_slot, child_idx, child_slot, …]`).
    pub(crate) fn slot_at_mut(&mut self, path: &[usize]) -> &mut ComponentSlot {
        match path {
            [i] => &mut self.components[*i],
            [child_idx, rest @ ..] => self.children[*child_idx].slot_at_mut(rest),
            [] => unreachable!("empty slot path"),
        }
    }

    /// The immutable twin of [`slot_at_mut`](Self::slot_at_mut).
    pub(crate) fn slot_at(&self, path: &[usize]) -> Option<&ComponentSlot> {
        match path {
            [i] => self.components.get(*i),
            [child_idx, rest @ ..] => self.children.get(*child_idx)?.slot_at(rest),
            [] => None,
        }
    }

    /// Collect `(priority, tree-path)` for every component in the subtree,
    /// pre-order; the engine sorts the flat list by priority.
    pub(crate) fn collect_order(&self, path: &mut Vec<usize>, out: &mut Vec<(i64, Vec<usize>)>) {
        if !self.enabled {
            return;
        }
        for (i, slot) in self.components.iter().enumerate() {
            path.push(i);
            out.push((slot.component.priority().value(), path.clone()));
            path.pop();
        }
        for (ci, child) in self.children.iter().enumerate() {
            path.push(ci);
            child.collect_order(path, out);
            path.pop();
        }
    }
}
