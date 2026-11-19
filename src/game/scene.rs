//! The scene document model — `.hyg` scenes for the game module. Pure data
//! plus the entity/component materialization; the whole thing runs and
//! tests headless.
//!
//! # The document
//!
//! A scene is a serialized `Var` riding the `.hyg` carrier
//! (`level_1.json.hyg` while authoring — diffable, mergeable, greppable):
//!
//! ```json
//! {
//!   "scene": "level_1",
//!   "format": 1,
//!   "camera": {
//!     "position": [0, 4, 0],
//!     "target": [0, 0, 0],
//!     "projection": { "ortho": { "half_extents": [8.89, 5, 64] } }
//!   },
//!   "entities": [
//!     {
//!       "name": "stack_block_3",
//!       "enabled": true,
//!       "transform": { "position": [0, 2.1, 0], "rotation": [0, 0, 0, 1], "scale": [1, 1, 1] },
//!       "children": [],
//!       "components": [
//!         { "type": "physics_body", "shape": "box", "half_extents": [0.5, 0.5, 0.5], "mass": 1.0 }
//!       ]
//!     }
//!   ]
//! }
//! ```
//!
//! Vectors are number lists (the [`SceneVec2`]/[`SceneVec3`]/[`SceneQuat`]
//! vocabulary reads them, and the native `Var` math variants too); numbers
//! are JSON numbers — `0` and `0.0` are the same value. The `camera` is the
//! scene's view (absent = the engine default); `entities` are the roots of
//! the entity tree, in order. Entity names may repeat (the engine's
//! find-first semantics) and may be any non-empty string.
//!
//! # Loading, saving, byte-identical round trips
//!
//! Loading validates with `FromVar` (strict, path-tracked: a bad scene
//! fails with `"entities.0.components.0.mass": expected a number, found
//! string`, never silently) and materializes components through the
//! **component registry** — type name → factory, pre-seeded with the
//! built-ins (`light`, `mesh_renderer`, `physics_body`, and `script` behind
//! the `script` feature). Plugins and user code register their own with
//! [`register_component_factory`]. Unknown component types are explicit
//! errors naming what IS registered.
//!
//! Saving serializes the **live tree** — the file always reflects current
//! state (physics poses, scripted motion), never the document it was loaded
//! from. The contract that makes scenes git-clean: **save → load → save is
//! byte-identical**. Saving is a canonical, deterministic function of typed
//! state: floats print their f32-shortest decimal (`2.1` stays `2.1`), and
//! that decimal parses back onto the same f32 through the f64 hop. Unknown
//! extra fields are tolerated on read, dropped on regeneration — the
//! canonical document is what the next save writes.
//!
//! Components that carry no scene schema (`scene_type` `None`) or no
//! expressible params in their current state are runtime-only: loading
//! never materializes them from a document, and saving skips them — the
//! component is code, not data. A model-backed `mesh_renderer` is
//! runtime-only until the asset pipeline gives models provenance.
//!
//! ```
//! use hyg::game::scene::Scene;
//! use hyg::game::{Entity, Light};
//!
//! // build
//! let mut scene = Scene::new("doctest");
//! let mut entity = Entity::new("sun");
//! entity.add_component(Box::new(Light::default()));
//! scene.entities.push(entity);
//!
//! // round trip through the document Var — canonical on both sides
//! let doc = scene.to_var();
//! let loaded = Scene::from_var(&doc).unwrap();
//! assert_eq!(loaded.to_var(), doc);
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use super::camera::{Camera, Projection};
use super::component::Component;
use super::engine::Engine;
use super::entity::Entity;
use super::light::Light;
use super::physics_body::PhysicsBody;
use super::renderable::MeshRenderer;
use super::transform::Transform;
#[cfg(feature = "script")]
use super::scripted::ScriptedComponent;
use crate::list::List;
use crate::o;
use crate::object::Object;
use crate::var::{Quat, Ty, Var, Vec3};
use crate::{FromVar, FromVarError};

/// The scene schema version this model reads and writes. A document with a
/// different `format` is an explicit error — migrations are the versioning
/// path, not silent best-effort (the graph model's contract, shared).
pub const SCENE_FORMAT: i64 = 1;

/// Per-entity context handed to component factories and scene serializers:
/// the entity a component hangs on, and the base directory file-referencing
/// params resolve against (script sources now; asset paths when the asset
/// pipeline lands). `Scene::load` sets `base` to the scene file's directory;
/// a bare [`Scene::from_var`] uses the working directory.
pub struct ComponentContext {
    /// The name of the entity the component hangs on (a physics body's
    /// default body name, for one).
    pub entity_name: String,
    /// The base directory for file-referencing params.
    pub base: PathBuf,
}

impl ComponentContext {
    /// A context with the default base (the working directory).
    pub fn new(entity_name: impl Into<String>) -> Self {
        ComponentContext {
            entity_name: entity_name.into(),
            base: PathBuf::from("."),
        }
    }
}

/// A component factory: the component's document params in, a live
/// component out. Errors carry `FromVar` paths relative to the params
/// object (`"mass": expected a number…`) — the scene walk extends them
/// with the entity path.
pub type ComponentFactory = Arc<
    dyn Fn(&Var, &ComponentContext) -> Result<Box<dyn Component>, FromVarError> + Send + Sync,
>;

type FactoryMap = HashMap<String, ComponentFactory>;

fn factories() -> &'static RwLock<FactoryMap> {
    static FACTORIES: OnceLock<RwLock<FactoryMap>> = OnceLock::new();
    FACTORIES.get_or_init(|| {
        let mut map: FactoryMap = HashMap::new();
        map.insert(
            "light".to_string(),
            Arc::new(|params, _ctx| {
                Light::from_params(params).map(|c| Box::new(c) as Box<dyn Component>)
            }),
        );
        map.insert(
            "physics_body".to_string(),
            Arc::new(|params, ctx| {
                PhysicsBody::from_params(params, &ctx.entity_name)
                    .map(|c| Box::new(c) as Box<dyn Component>)
            }),
        );
        map.insert(
            "mesh_renderer".to_string(),
            Arc::new(|params, ctx| {
                let mut renderer = MeshRenderer::from_params(params)?;
                // asset-backed renderers materialize at load time, path
                // resolved against the scene's directory (the same rule
                // the script source path follows). Without the `gltf`
                // feature the renderer stays unresolved — it round-trips
                // by path and draws nothing until a build that can load
                // it.
                #[cfg(feature = "gltf")]
                renderer
                    .resolve(&ctx.base)
                    .map_err(|e| crate::FromVarError::new(e).at("model"))?;
                Ok(Box::new(renderer) as Box<dyn Component>)
            }),
        );
        map.insert(
            "camera".to_string(),
            Arc::new(|params, _ctx| {
                crate::game::camera_component::CameraComponent::from_params(params)
                    .map(|c| Box::new(c) as Box<dyn Component>)
            }),
        );
        #[cfg(feature = "script")]
        map.insert(
            "script".to_string(),
            Arc::new(|params, ctx| {
                ScriptedComponent::from_scene_params(params, &ctx.base)
                    .map(|c| Box::new(c) as Box<dyn Component>)
            }),
        );
        RwLock::new(map)
    })
}

/// Register a component factory under a document type name (the `type`
/// field its document objects carry). Re-registering replaces — the
/// registry pattern every hyg backend registry follows. Built-ins are
/// pre-seeded; plugins and user code register their own.
pub fn register_component_factory(type_name: &str, factory: ComponentFactory) {
    factories().write().unwrap().insert(type_name.to_string(), factory);
}

/// Materialize a component from its document object (`{ "type": …, … }`)
/// through the registry.
///
/// # Errors
/// An unknown type (naming what IS registered), or the factory's own
/// `FromVar` failure.
pub fn create_component(
    type_name: &str,
    params: &Var,
    ctx: &ComponentContext,
) -> Result<Box<dyn Component>, FromVarError> {
    // scope the guard, THEN call — a factory must never run under the
    // registry lock (the registry-reads-itself deadlock pattern)
    let factory = factories().read().unwrap().get(type_name).cloned();
    match factory {
        Some(factory) => factory(params, ctx),
        None => {
            let known = registered_component_types();
            Err(FromVarError::new(format!(
                "unknown component type {type_name:?} (registered: {known:?})"
            )))
        }
    }
}

/// The registered component type names, sorted (stable error messages and
/// editor listings).
pub fn registered_component_types() -> Vec<String> {
    let mut names: Vec<String> = factories()
        .read()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

// ---- the scene number vocabulary ----
//
// Vectors in the document are number lists; these newtypes are the strict
// readers behind them. Values are strict — numbers only (JSON's one number
// type, int or float), exact arity. Saving always writes lists.

/// A strict scene number: int or float, never bool/string.
pub(crate) fn scene_f32(v: &Var) -> Result<f32, FromVarError> {
    match v {
        Var::I64(n) => Ok(*n as f32),
        Var::F64(n) => Ok(*n as f32),
        other => Err(FromVarError::new(format!(
            "expected a number, found {}",
            other.ty()
        ))),
    }
}

/// The numbers of a scene list: exactly `count` of them.
fn scene_number_list(v: &Var, count: usize) -> Result<Vec<f32>, FromVarError> {
    let list = v.as_list().ok_or_else(|| {
        FromVarError::new(format!("expected a list of {count} numbers, found {}", v.ty()))
    })?;
    if list.len() != count {
        return Err(FromVarError::new(format!(
            "expected a list of {count} numbers, got {}",
            list.len()
        )));
    }
    list.iter().iter().map(scene_f32).collect()
}

/// The document's `[a, b]` — a 2-number list (light cones).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneVec2(pub [f32; 2]);

impl FromVar for SceneVec2 {
    fn from_var(v: &Var) -> Result<Self, FromVarError> {
        let items = scene_number_list(v, 2)?;
        Ok(SceneVec2([items[0], items[1]]))
    }
}

/// The document's `[x, y, z]` — a 3-number list (or a native `Var::Vec3`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneVec3(pub Vec3);

impl FromVar for SceneVec3 {
    fn from_var(v: &Var) -> Result<Self, FromVarError> {
        if let Var::Vec3(m) = v {
            return Ok(SceneVec3(*m));
        }
        let items = scene_number_list(v, 3)?;
        Ok(SceneVec3(Vec3::new(items[0], items[1], items[2])))
    }
}

/// The document's `[x, y, z, w]` — a 4-number list (or a native
/// `Var::Vec4`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneVec4(pub [f32; 4]);

impl FromVar for SceneVec4 {
    fn from_var(v: &Var) -> Result<Self, FromVarError> {
        if let Var::Vec4(m) = v {
            return Ok(SceneVec4([m.x, m.y, m.z, m.w]));
        }
        let items = scene_number_list(v, 4)?;
        Ok(SceneVec4([items[0], items[1], items[2], items[3]]))
    }
}

/// The document's `[x, y, z, w]` rotation — a 4-number list (or a native
/// `Var::Quat`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneQuat(pub Quat);

impl FromVar for SceneQuat {
    fn from_var(v: &Var) -> Result<Self, FromVarError> {
        if let Var::Quat(m) = v {
            return Ok(SceneQuat(*m));
        }
        let items = scene_number_list(v, 4)?;
        Ok(SceneQuat(Quat::new(items[0], items[1], items[2], items[3])))
    }
}

// ---- strict field reads (the factories' shared vocabulary) ----

/// The field at `key`, absent-as-`Null` collapsed to `None` — the optional
/// field read (unknown extra fields are tolerated the same way, just
/// ignored entirely).
pub(crate) fn opt_field(v: &Var, key: &str) -> Option<Var> {
    let field = v.get(key);
    if matches!(field, Var::Null) { None } else { Some(field) }
}

/// An optional string field.
pub(crate) fn opt_string(v: &Var, key: &str) -> Result<Option<String>, FromVarError> {
    match opt_field(v, key) {
        None => Ok(None),
        Some(Var::Str(s)) => Ok(Some(s)),
        Some(other) => Err(
            FromVarError::new(format!("expected a string, found {}", other.ty())).at(key),
        ),
    }
}

/// A required non-empty string field.
pub(crate) fn req_string(v: &Var, key: &str) -> Result<String, FromVarError> {
    match v.get(key) {
        Var::Str(s) if !s.is_empty() => Ok(s),
        other => Err(FromVarError::new(format!(
            "\"{key}\" is required — a non-empty string, found {}",
            other.ty()
        ))
        .at(key)),
    }
}

/// An optional number field.
pub(crate) fn opt_f32(v: &Var, key: &str) -> Result<Option<f32>, FromVarError> {
    match opt_field(v, key) {
        None => Ok(None),
        Some(n) => scene_f32(&n).map(Some).map_err(|e| e.at(key)),
    }
}

// ---- the document structs ----

/// The document's transform object — every field optional, identity by
/// default, read through the derive (strict, path-tracked).
#[derive(FromVar)]
struct SceneTransformDoc {
    position: Option<SceneVec3>,
    rotation: Option<SceneQuat>,
    scale: Option<SceneVec3>,
}

impl SceneTransformDoc {
    fn into_transform(self) -> Transform {
        let mut t = Transform::default();
        if let Some(p) = self.position {
            t.position = p.0;
        }
        if let Some(r) = self.rotation {
            t.rotation = r.0;
        }
        if let Some(s) = self.scale {
            t.scale = s.0;
        }
        t
    }
}

/// The document's camera object — fields optional, engine default where
/// absent. The projection is the tagged encoding `{"ortho": …}` /
/// `{"perspective": …}` ([`Projection`]'s `FromVar`).
#[derive(FromVar)]
struct CameraDoc {
    position: Option<SceneVec3>,
    target: Option<SceneVec3>,
    projection: Option<Projection>,
}

fn parse_camera(v: &Var) -> Result<Camera, FromVarError> {
    let doc = CameraDoc::from_var(v).map_err(|e| e.at("camera"))?;
    let default = Camera::default();
    Ok(Camera {
        position: doc.position.map(|s| s.0).unwrap_or(default.position),
        target: doc.target.map(|s| s.0).unwrap_or(default.target),
        projection: doc.projection.unwrap_or(default.projection),
    })
}

/// Why a scene file failed to load.
#[derive(Debug)]
pub enum SceneLoadError {
    /// The carrier refused the file (not `.hyg`, unknown format, bad
    /// bytes, filesystem trouble).
    Carrier(crate::carrier::CarrierError),
    /// The document didn't match the scene schema — the `FromVar` path
    /// names where.
    Invalid(FromVarError),
}

impl std::fmt::Display for SceneLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SceneLoadError::Carrier(e) => write!(f, "{e}"),
            SceneLoadError::Invalid(e) => write!(f, "invalid scene: {e}"),
        }
    }
}

impl std::error::Error for SceneLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SceneLoadError::Carrier(e) => e.source(),
            SceneLoadError::Invalid(e) => Some(e),
        }
    }
}

impl From<crate::carrier::CarrierError> for SceneLoadError {
    fn from(e: crate::carrier::CarrierError) -> Self {
        SceneLoadError::Carrier(e)
    }
}

/// A scene: the name, the camera the scene renders through, and the roots
/// of the materialized entity tree (in document order).
pub struct Scene {
    /// The scene's name (`"scene"` in the document).
    pub name: String,
    /// The camera the scene renders through (`"camera"`; the engine
    /// default where the document omitted it).
    pub camera: Camera,
    /// The root entities (`"entities"`), in document order.
    pub entities: Vec<Entity>,
}

impl Scene {
    /// An empty scene with the engine-default camera.
    pub fn new(name: impl Into<String>) -> Scene {
        Scene {
            name: name.into(),
            camera: Camera::default(),
            entities: Vec::new(),
        }
    }

    /// Parse and validate a scene document, materializing the entity tree
    /// and its components through the registry. File references in
    /// component params resolve against the working directory — use
    /// [`Scene::load`] for scenes that name files next to themselves.
    ///
    /// # Errors
    /// A [`FromVarError`] whose path names the offending field
    /// (`"entities.0.components.0.mass"`), or the format gate.
    pub fn from_var(doc: &Var) -> Result<Scene, FromVarError> {
        Scene::from_var_with_base(doc, PathBuf::from("."))
    }

    fn from_var_with_base(doc: &Var, base: PathBuf) -> Result<Scene, FromVarError> {
        if !matches!(doc.ty(), Ty::Object) {
            return Err(FromVarError::new(format!(
                "scene document: expected an object, found {}",
                doc.ty()
            )));
        }

        // the name — a non-empty string
        let name = doc
            .get("scene")
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                FromVarError::new(
                    "scene document: \"scene\" (the name) is required — a non-empty string",
                )
            })?
            .to_string();

        // the format gate — the versioning hook
        let format = doc.get("format").as_i64().ok_or_else(|| {
            FromVarError::new("scene document: \"format\" is required — an integer")
        })?;
        if format != SCENE_FORMAT {
            return Err(FromVarError::new(format!(
                "\"format\": {format} — this hyg speaks scene format {SCENE_FORMAT}"
            )));
        }

        // the camera — optional, engine default where absent
        let camera = match doc.get("camera") {
            Var::Null => Camera::default(),
            cam => parse_camera(&cam)?,
        };

        // the roots — optional (an empty scene is a valid scene)
        let mut entities = Vec::new();
        let roots = doc.get("entities");
        if !matches!(roots, Var::Null) {
            let items = roots.as_list().ok_or_else(|| {
                FromVarError::new(format!("\"entities\": expected a list, found {}", roots.ty()))
            })?;
            for (i, item) in items.iter().iter().enumerate() {
                entities.push(parse_entity(item, &format!("entities.{i}"), &base)?);
            }
        }

        Ok(Scene {
            name,
            camera,
            entities,
        })
    }

    /// Load a scene from a `.hyg` file (the carrier picks the format from
    /// the filename). File references in component params resolve against
    /// the scene file's directory.
    ///
    /// # Errors
    /// [`SceneLoadError::Carrier`] for carrier-level trouble, or
    /// [`SceneLoadError::Invalid`] with the `FromVar` path of the first
    /// schema failure.
    pub fn load(path: &Path) -> Result<Scene, SceneLoadError> {
        let doc = crate::carrier::read(path)?;
        let base = match path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
            _ => PathBuf::from("."),
        };
        Scene::from_var_with_base(&doc, base).map_err(SceneLoadError::Invalid)
    }

    /// Save the scene to a `.hyg` file — the canonical document of the
    /// current (typed) state, the format picked from the filename.
    ///
    /// # Errors
    /// The carrier's (see [`crate::carrier::write`]).
    pub fn save(&self, path: &Path) -> Result<(), crate::carrier::CarrierError> {
        crate::carrier::write(path, &self.to_var())
    }

    /// The canonical document `Var` for a live entity tree — the editor's
    /// save seam: hand it the engine's roots and the engine's camera and
    /// write the result wherever the scene lives. Schema field order
    /// (`scene`, `format`, `camera`, `entities`); unknown extras are
    /// dropped, defaults omitted in component params.
    #[must_use]
    pub fn document(name: &str, camera: &Camera, entities: &[Entity]) -> Var {
        Var::from(o! {
            "scene": Var::from(name),
            "format": Var::from(SCENE_FORMAT),
            "camera": camera_to_var(camera),
            "entities": Var::from(List::from_iter(entities.iter().map(entity_to_var))),
        })
    }

    /// The canonical document of this scene.
    #[must_use]
    pub fn to_var(&self) -> Var {
        Self::document(&self.name, &self.camera, &self.entities)
    }

    /// Materialize into an engine: the camera attached, the roots added in
    /// order. A scene spawns once — the engine owns the tree after.
    pub fn spawn_into(self, engine: &mut Engine) {
        engine.set_camera(self.camera);
        for entity in self.entities {
            engine.add_entity(entity);
        }
    }
}

/// Parse and materialize one entity (recursing through `children`).
/// `path` is the composed document path so far (`entities.0.children.2`) —
/// every failure names its field through it.
fn parse_entity(v: &Var, path: &str, base: &Path) -> Result<Entity, FromVarError> {
    if !matches!(v.ty(), Ty::Object) {
        return Err(
            FromVarError::new(format!("expected an object, found {}", v.ty())).at(path),
        );
    }

    // name — the script-facing handle; any non-empty string
    let name = v
        .get("name")
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            FromVarError::new(
                "\"name\" is required — a non-empty string (the entity's script-facing handle)",
            )
            .at(format!("{path}.name"))
        })?
        .to_string();
    let mut entity = Entity::new(name.clone());

    // enabled — optional, default true
    match v.get("enabled") {
        Var::Null => {}
        Var::Bool(b) => entity.set_enabled(b),
        other => {
            return Err(FromVarError::new(format!("expected bool, found {}", other.ty()))
                .at(format!("{path}.enabled")))
        }
    }

    // transform — optional, default identity
    match v.get("transform") {
        Var::Null => {}
        transform => {
            let doc = SceneTransformDoc::from_var(&transform)
                .map_err(|e| e.at(format!("{path}.transform")))?;
            *entity.transform_mut() = doc.into_transform();
        }
    }

    // children — optional, recursed
    match v.get("children") {
        Var::Null => {}
        children => {
            let list = children.as_list().ok_or_else(|| {
                FromVarError::new(format!("expected a list, found {}", children.ty()))
                    .at(format!("{path}.children"))
            })?;
            for (i, child) in list.iter().iter().enumerate() {
                entity.add_child(parse_entity(child, &format!("{path}.children.{i}"), base)?);
            }
        }
    }

    // components — optional, materialized through the registry
    match v.get("components") {
        Var::Null => {}
        components => {
            let list = components.as_list().ok_or_else(|| {
                FromVarError::new(format!("expected a list, found {}", components.ty()))
                    .at(format!("{path}.components"))
            })?;
            for (i, comp) in list.iter().iter().enumerate() {
                let comp_path = format!("{path}.components.{i}");
                if !matches!(comp.ty(), Ty::Object) {
                    return Err(FromVarError::new(format!(
                        "expected an object, found {}",
                        comp.ty()
                    ))
                    .at(&comp_path));
                }
                let type_var = comp.get("type");
                let type_name = type_var
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        FromVarError::new(
                            "\"type\" is required — the component's registry name (\"light\", \"physics_body\", …)",
                        )
                        .at(format!("{comp_path}.type"))
                    })?;
                let ctx = ComponentContext {
                    entity_name: name.clone(),
                    base: base.to_path_buf(),
                };
                let component = create_component(type_name, comp, &ctx)
                    .map_err(|e| e.at(&comp_path))?;
                entity.add_component(component);
            }
        }
    }

    Ok(entity)
}

// ---- the save walk ----

/// One entity's document object — the schema fields, always written:
/// `name`, `enabled`, `transform` (all three fields), `children`,
/// `components`.
fn entity_to_var(entity: &Entity) -> Var {
    let transform = entity.transform();
    let children: List = entity.children().iter().map(entity_to_var).collect();
    let components: List = entity
        .iter_components()
        .filter_map(|c| component_to_var(c, entity.name()))
        .collect();
    Var::from(o! {
        "name": Var::from(entity.name()),
        "enabled": Var::from(entity.enabled()),
        "transform": Var::from(o! {
            "position": vec3_to_var(transform.position),
            "rotation": quat_to_var(transform.rotation),
            "scale": vec3_to_var(transform.scale),
        }),
        "children": Var::from(children),
        "components": Var::from(components),
    })
}

/// One component's document object: `{"type": …, …params}`. Components
/// with no scene schema ([`Component::scene_type`] `None`) or no
/// expressible params in their current state
/// ([`Component::scene_params`] `None`) are runtime-only and skipped —
/// the scene keeps what scenes can express.
fn component_to_var(component: &dyn Component, entity_name: &str) -> Option<Var> {
    let type_name = component.scene_type()?;
    let params = component.scene_params(&ComponentContext::new(entity_name))?;
    let obj = Object::new();
    obj.set("type", Var::from(type_name));
    if let Some(entries) = params.as_object() {
        for (key, value) in entries.entries() {
            obj.set(key, value);
        }
    }
    Some(Var::from(obj))
}

/// The camera's document object — all fields, always written.
fn camera_to_var(camera: &Camera) -> Var {
    let projection = match camera.projection {
        Projection::Orthographic { half_extents } => Var::from(o! {
            "ortho": Var::from(o! { "half_extents": vec3_to_var(half_extents) }),
        }),
        Projection::Perspective {
            fov_y_degrees,
            near,
            far,
            aspect,
        } => Var::from(o! {
            "perspective": Var::from(o! {
                "fov": f32_to_var(fov_y_degrees),
                "near": f32_to_var(near),
                "far": f32_to_var(far),
                "aspect": f32_to_var(aspect),
            }),
        }),
    };
    Var::from(o! {
        "position": vec3_to_var(camera.position),
        "target": vec3_to_var(camera.target),
        "projection": projection,
    })
}

/// A float as the document writes it: the f32's shortest decimal, held as
/// f64. Widening naively would print `2.1f32` as `2.0999999046325684`; its
/// shortest decimal `2.1` parses (via f64) back onto the same f32 — the
/// f32 round-trip decimal survives the wider hop — so save → load → save
/// is byte-identical AND scenes stay readable.
///
/// Non-finite floats have no shortest decimal (and JSON has no encoding):
/// they fall through to the f64, which serde_json writes as `null` — the
/// loader then rejects the field loudly ("expected a number"), never
/// silently.
pub(crate) fn f32_to_var(n: f32) -> Var {
    match format!("{n}").parse::<f64>() {
        Ok(d) => Var::F64(d),
        Err(_) => Var::F64(n as f64),
    }
}

/// A float list as a number list (colors, cones).
pub(crate) fn f32s_to_var(values: &[f32]) -> Var {
    Var::from(List::from_iter(values.iter().map(|&n| f32_to_var(n))))
}

/// A vec3 as an `[x, y, z]` list — the scene layer's vector encoding.
pub(crate) fn vec3_to_var(v: Vec3) -> Var {
    f32s_to_var(&[v.x, v.y, v.z])
}

/// A quaternion as an `[x, y, z, w]` list.
pub(crate) fn quat_to_var(q: Quat) -> Var {
    f32s_to_var(&[q.x, q.y, q.z, q.w])
}
