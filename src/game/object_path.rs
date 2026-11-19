//! ObjectPaths — stable STRING references to entities and components,
//! the Godot NodePath shape for hyg's tree.
//!
//! A path is what a scene document WRITES (a string in JSON) and what
//! code RESOLVES at runtime — the knob between the two. Absolute paths
//! anchor at the scene's roots; relative paths anchor at the
//! referencing entity (`..` climbs, bare names descend, `.` is self).
//! A `:type` suffix names a COMPONENT on the resolved entity, using
//! the component's document name (the `scene_type` vocabulary —
//! `"physics_body"`, `"mesh_renderer"`, …), so documents stay honest
//! without knowing Rust types.
//!
//! ```text
//! /world/stack            absolute: root "world", child "stack"
//! ../sibling              the referencing entity's parent, then "sibling"
//! ./turret                a child of self (same as "turret")
//! /player:physics_body    the physics body component on "/player"
//! ```
//!
//! Names match FIRST (documented: duplicate sibling names resolve to
//! the first match in tree order). Paths are not stored indices —
//! indices shuffle when trees re-order; names survive.

use crate::game::component::Component;
use crate::game::Engine;
use crate::{FromVar, FromVarError};
use crate::var::Var;

/// One step of an [`ObjectPath`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// A named entity to descend into.
    Name(String),
    /// Climb to the parent entity.
    Parent,
    /// Stay on the current entity (an explicit `.`).
    Self_,
}

/// A parsed reference to an entity (or one of its components): a
/// segment walk plus an optional component type name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectPath {
    /// Absolute paths anchor at the scene's roots; relative at the
    /// referencing entity.
    absolute: bool,
    segments: Vec<Segment>,
    component: Option<String>,
}

/// Why a path string didn't parse — the message names the position and
/// the rule (the author-facing voice: documents fail with the fix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathError {
    pub message: String,
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PathError {}

impl ObjectPath {
    /// Parse a path string. The grammar:
    ///
    /// - `/a/b` — absolute (leading slash; leading slashes beyond the
    ///   first are normalized)
    /// - `a`, `./a`, `../a`, `../../a` — relative (`..` climbs, `.` is
    ///   self, a bare name is a child)
    /// - `path:type` — the `:` suffix (after any walk) names a
    ///   component by its document type; empty type names are errors
    /// - entity names may contain anything except `/` and `:` and may
    ///   not be empty
    ///
    /// # Errors
    /// [`PathError`] naming the offending position.
    pub fn parse(text: &str) -> Result<ObjectPath, PathError> {
        let (walk, component) = match text.split_once(':') {
            Some((walk, component)) => (walk, Some(component)),
            None => (text, None),
        };
        let component = match component {
            Some(name) if name.is_empty() || name.contains('/') => {
                return Err(PathError {
                    message: format!(
                        "{text:?}: the type after `:` must be a plain component name"
                    ),
                });
            }
            other => other.map(str::to_string),
        };

        let absolute = walk.starts_with('/');
        let mut segments: Vec<Segment> = Vec::new();
        for (index, raw) in walk.split('/').enumerate() {
            if index == 0 && raw.is_empty() {
                // the leading slash's empty left side (further slashes
                // are normalized empty segments — skip them)
                continue;
            }
            match raw {
                "" => {} // doubled slashes: normalized
                "." => segments.push(Segment::Self_),
                ".." => segments.push(Segment::Parent),
                name => {
                    segments.push(Segment::Name(name.to_string()));
                }
            }
        }

        if segments.is_empty() && !absolute {
            return Err(PathError {
                message: format!("{text:?}: an empty path names nothing"),
            });
        }
        if segments.is_empty() {
            // "/" alone: absolute self — legal but pointless; keep it
            // (it resolves to the first root) rather than special-case
        }
        for segment in &segments {
            if let Segment::Name(name) = segment {
                if name.is_empty() {
                    return Err(PathError {
                        message: format!("{text:?}: entity names cannot be empty"),
                    });
                }
            }
        }
        Ok(ObjectPath {
            absolute,
            segments,
            component,
        })
    }

    /// Whether the path anchors at the scene's roots (true) or the
    /// referencing entity (false).
    #[must_use]
    pub fn is_absolute(&self) -> bool {
        self.absolute
    }

    /// The walk's segments (the introspection tests assert the parse
    /// produced the expected steps).
    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// The component type name this path points at, if any.
    #[must_use]
    pub fn component_type(&self) -> Option<&str> {
        self.component.as_deref()
    }

    /// The walk back to text — exact round trip through
    /// [`parse`](Self::parse).
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        if self.absolute {
            text.push('/');
        }
        for (index, segment) in self.segments.iter().enumerate() {
            if index > 0 {
                text.push('/');
            }
            match segment {
                Segment::Name(name) => text.push_str(name),
                Segment::Parent => text.push_str(".."),
                Segment::Self_ => text.push('.'),
            }
        }
        if let Some(component) = &self.component {
            text.push(':');
            text.push_str(component);
        }
        text
    }

    /// Resolve against a tree: `roots` are the scene's root entities,
    /// `origin` is the referencing entity's index path (the root index,
    /// then child indices — the shape
    /// [`Engine::entity_at`](crate::game::Engine::entity_at) takes).
    /// Returns the resolved entity's index path, same shape.
    ///
    /// Absolute paths match a ROOT by name, then descend; relative
    /// paths anchor at `origin` (climbing above a root resolves to
    /// nothing — there is no parent of the scene). Names match FIRST
    /// in tree order.
    #[must_use]
    pub fn resolve(
        &self,
        roots: &[crate::game::Entity],
        origin: &[usize],
    ) -> Option<Vec<usize>> {
        // the working position: Some(walk) = the entity at that index
        // walk; None = unpositioned (an absolute path before its first
        // named segment)
        fn descend<'a>(roots: &'a [crate::game::Entity], walk: &[usize]) -> Option<&'a crate::game::Entity> {
            let mut entity = roots.get(*walk.first()?)?;
            for index in &walk[1..] {
                entity = entity.children().get(*index)?;
            }
            Some(entity)
        }

        let mut walk: Option<Vec<usize>> = if self.absolute {
            None
        } else {
            Some(origin.to_vec())
        };

        for segment in &self.segments {
            match segment {
                Segment::Self_ => {
                    // absolute "." anchors at the first root
                    if walk.is_none() {
                        walk = Some(vec![0]);
                    }
                }
                // climbing: a None walk (unpositioned) or an empty walk
                // (above a root — the scene has no parent) bails
                Segment::Parent => {
                    walk.as_mut()?.pop()?;
                }
                Segment::Name(name) => {
                    let index = match &walk {
                        // absolute: match a ROOT by name
                        None => roots.iter().position(|root| root.name() == name)?,
                        // relative: descend the CURRENT entity's children
                        Some(position) => {
                            let entity = descend(roots, position)?;
                            entity
                                .children()
                                .iter()
                                .position(|child| child.name() == name)?
                        }
                    };
                    match &mut walk {
                        None => walk = Some(vec![index]),
                        Some(position) => position.push(index),
                    }
                }
            }
        }

        match walk {
            Some(walk) if !walk.is_empty() => Some(walk),
            // absolute "/" or "." with nothing walked: the first root
            None if self.absolute && self.segments.is_empty() => Some(vec![0]),
            // anything else (a trailing climb above a root, a relative
            // self at an empty origin) points nowhere
            _ => None,
        }
    }
}


impl FromVar for ObjectPath {
    fn from_var(v: &Var) -> Result<Self, FromVarError> {
        let text = v.as_str().ok_or_else(|| {
            FromVarError::new(format!("expected a path string, found {}", v.ty()))
        })?;
        ObjectPath::parse(text)
            .map_err(|e| FromVarError::new(e.message))
    }
}

impl Engine {
    /// Resolve an ObjectPath from an origin entity (its index path)
    /// against this engine's tree — the resolved entity's index path
    /// (the same shape [`Engine::entity_at`](Self::entity_at) takes).
    #[must_use]
    pub fn resolve_object_path(
        &self,
        origin: &[usize],
        path: &ObjectPath,
    ) -> Option<Vec<usize>> {
        path.resolve(self.entities(), origin)
    }

    /// TEMPLATE HELPER — the GameObject a path names, from an origin.
    /// `path` is the raw string (parse errors resolve to `None`):
    /// `engine.entity_at_object_path(&[0], "../turret")`.
    #[must_use]
    pub fn entity_at_object_path(&self, origin: &[usize], path: &str) -> Option<&crate::game::Entity> {
        let parsed = ObjectPath::parse(path).ok()?;
        let resolved = self.resolve_object_path(origin, &parsed)?;
        self.entity_at(&resolved)
    }

    /// TEMPLATE HELPER — the COMPONENT a path names, typed. The `:type`
    /// suffix (when present) must agree with the requested component's
    /// document type; the resolved entity must carry it.
    /// `engine.component_at_object_path::<PhysicsBody>(&[0], "../ball:physics_body")`.
    #[must_use]
    pub fn component_at_object_path<T: Component + 'static>(
        &self,
        origin: &[usize],
        path: &str,
    ) -> Option<&T> {
        let parsed = ObjectPath::parse(path).ok()?;
        if let Some(wanted) = parsed.component_type() {
            let resolved = self.resolve_object_path(origin, &parsed)?;
            let entity = self.entity_at(&resolved)?;
            let carried = entity
                .iter_components()
                .any(|c| c.scene_type() == Some(wanted));
            if !carried {
                return None;
            }
        }
        self.component_at_object_path_untyped::<T>(origin, &parsed)
    }

    /// The typed component on the path's entity, ignoring any `:type`
    /// suffix (the suffix check runs first in the typed helper above).
    fn component_at_object_path_untyped<T: Component + 'static>(
        &self,
        origin: &[usize],
        parsed: &ObjectPath,
    ) -> Option<&T> {
        let resolved = self.resolve_object_path(origin, parsed)?;
        let entity = self.entity_at(&resolved)?;
        entity
            .iter_components()
            .filter_map(|c| c.as_any().downcast_ref::<T>())
            .next()
    }
}
