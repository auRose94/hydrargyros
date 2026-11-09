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
}

impl Entity {
    pub fn new(name: impl Into<String>) -> Entity {
        Entity {
            name: name.into(),
            transform: Transform::default(),
            components: Vec::new(),
            children: Vec::new(),
            enabled: true,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
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

    pub fn children(&self) -> &[Entity] {
        &self.children
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
