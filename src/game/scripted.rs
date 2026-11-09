//! Scripted components — the interpreter seam meeting the game loop.
//!
//! A [`ScriptedComponent`] wraps a [`Script`](crate::script::Script) handle
//! (the `script` feature, v12 underneath) and dispatches the optional
//! component lifecycle to script functions: `initialize()` →
//! `script.initialize()`, `update(dt)` → `script.update(dt)`, `draw()` →
//! `script.draw()`. Missing script functions are silently skipped —
//! optional-lifecycle semantics (a component that only draws defines only
//! `draw`). A script that *throws* fails loud: the pump would surface an
//! uncaught throw, and a Rust `Component` has no error channel, so the error
//! panics with the script error attached.
//!
//! # The transform seam
//!
//! `Component::update(&mut self, dt)` gets no entity access (the trait is
//! deliberately shape-of-a-callback), yet the whole point of a scripted
//! component is `update(dt)` moving its entity — and script code cannot
//! reach the entity directly, because entities are not script objects. The
//! component carries the entity transform as a **shared handle**:
//! `Arc<Mutex<Transform>>`. The script sees it as a global object named
//! `transform` — `{position: {x, y, z}, rotation: {x, y, z, w}, scale:
//! {x, y, z}}` — refreshed from the handle before each hook and written
//! back after it (the same read-modify-write the engine's physics sync
//! does, at the component instead of in the pump). One `Script` drives one
//! `ScriptedComponent`: the script's `transform` global IS the component's
//! entity transform.
//!
//! The engine-side link is landed, and the handle is the only writer: after
//! every component pass the engine syncs each scripted component's handle
//! into its entity's transform (`Engine::script_sync`, the physics sync's
//! sibling), and an unseeded component spawns at its ENTITY's transform —
//! the physics-body rule — so incremental scripts start from where the
//! entity was placed. One mover per entity: physics XOR script — an entity
//! carrying both a `PhysicsBody` and a `ScriptedComponent` syncs in
//! undefined order (the physics pose lands first, the script's write-back
//! overwrites it); don't rely on it.
//!
//! The bridge between hooks and script is a single compiled bootstrap —
//! the guard is compiled once and called per frame through
//! [`Script::call_unit`]'s scalar fast path (no JSON
//! round-trip on the hot path, no recompile per frame):
//!
//! ```js
//! function __v12_frame(dt) {
//!     globalThis.transform = JSON.parse(__v12_transform_get());
//!     if (typeof update === 'function') { update(dt); }
//!     __v12_transform_set(JSON.stringify(globalThis.transform));
//! }
//! ```
//!
//! (shown expanded; the real shims read the configured hook names through a
//! host fn, so `HookNames` is customizable). The `__v12_transform_*` host
//! fns close over the shared handle and decode/encode through the same
//! explicit `Var` conversion path as everything else — with path-style
//! errors when the script breaks the `transform` shape.

use std::any::Any;
use std::sync::{Arc, Mutex};

use super::component::{Component, Priority};
use super::transform::Transform;
use crate::lock;
use crate::script::{Script, ScriptError};
use crate::var::{Quat, Var, Vec3};

/// The script function names the lifecycle dispatches to — the standard
/// hook names by default, renamable so two scripted components can share one
/// script's functions (e.g. `"tick"` instead of `"update"`).
#[derive(Debug, Clone)]
pub struct HookNames {
    /// Called once, the first frame the component exists in the engine.
    pub initialize: String,
    /// Called every frame with the delta time in seconds.
    pub update: String,
    /// Called in the draw pass.
    pub draw: String,
}

impl Default for HookNames {
    fn default() -> Self {
        HookNames {
            initialize: "initialize".to_string(),
            update: "update".to_string(),
            draw: "draw".to_string(),
        }
    }
}

/// A component driven by a v12 script.
/// One script per component: the script's `transform` global is this
/// component's shared entity transform. Send + Sync through and through —
/// the `Script` is a handle; the `!Send` context lives in the script
/// module's thread-local pool.
pub struct ScriptedComponent {
    script: Script,
    /// The shared entity transform (see the module docs — the seam that
    /// lets `update(dt)` move the entity).
    transform: Arc<Mutex<Transform>>,
    hooks: HookNames,
    priority: Priority,
}

impl ScriptedComponent {
    /// A scripted component with the default hook names
    /// (`initialize`/`update`/`draw`) and a default transform.
    ///
    /// # Errors
    /// Arming the hook bootstrap on the script (a bridge failure here is a
    /// bug — the generated JS is fixed — but the constructor is honest
    /// about it).
    pub fn new(script: Script) -> Result<ScriptedComponent, ScriptError> {
        ScriptedComponent::with_hooks(script, HookNames::default())
    }

    /// A scripted component with custom hook names (armed before the
    /// bootstrap is generated — hook names are part of the generated JS).
    ///
    /// # Errors
    /// Same as [`new`](Self::new); invalid hook names also error here.
    pub fn with_hooks(script: Script, hooks: HookNames) -> Result<ScriptedComponent, ScriptError> {
        for (slot, name) in [
            ("initialize", &hooks.initialize),
            ("update", &hooks.update),
            ("draw", &hooks.draw),
        ] {
            crate::script::validate_identifier(name)
                .map_err(|e| ScriptError::Type(format!("hook {slot} name: {e}")))?;
        }
        let transform = Arc::new(Mutex::new(Transform::default()));
        let mut component = ScriptedComponent {
            script,
            transform: Arc::clone(&transform),
            hooks,
            priority: Priority::Generic,
        };
        component.arm_bridge(transform)?;
        Ok(component)
    }

    /// Start in a custom priority tier (default: `Priority::Generic`).
    #[must_use]
    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    /// Start from a specific transform (the shared handle's initial value).
    #[must_use]
    pub fn with_transform(self, transform: Transform) -> Self {
        *lock(&self.transform) = transform;
        self
    }

    /// The shared transform handle — the entity transform this component's
    /// script moves. Test/native code holds clones of this to read the
    /// scripted motion (the engine test does exactly that).
    #[must_use]
    pub fn transform_handle(&self) -> Arc<Mutex<Transform>> {
        Arc::clone(&self.transform)
    }

    /// A snapshot of the shared transform (poison-tolerant read).
    #[must_use]
    pub fn transform(&self) -> Transform {
        *lock(&self.transform)
    }

    /// Overwrite the shared transform (poison-tolerant write).
    pub fn set_transform(&self, transform: Transform) {
        *lock(&self.transform) = transform;
    }

    /// The script handle.
    #[must_use]
    pub fn script(&self) -> &Script {
        &self.script
    }

    /// The script handle, mutably (to register host fns, call back, …).
    pub fn script_mut(&mut self) -> &mut Script {
        &mut self.script
    }

    /// The configured hook names.
    #[must_use]
    pub fn hooks(&self) -> &HookNames {
        &self.hooks
    }

    /// Arm the per-component seam: the two transform host fns (closing over
    /// the shared handle) plus the compiled hook shims. This is the only
    /// component→script plumbing; everything else is plain `call_unit`.
    fn arm_bridge(&mut self, handle: Arc<Mutex<Transform>>) -> Result<(), ScriptError> {
        // read side: handle → the `transform` global's JSON (infallible —
        // floats to floats). The guard is held only for the copy-out; no
        // lock is held across JS.
        self.script.register_internal("__v12_transform_get", {
            let handle = Arc::clone(&handle);
            move |_args| Ok(transform_to_var(&lock(&handle)))
        })?;

        // write side: the `transform` global's JSON → handle. Decode fully
        // BEFORE taking the guard (path-style errors throw into JS, which
        // surfaces as a script error in the hook call).
        self.script.register_internal("__v12_transform_set", {
            let handle = Arc::clone(&handle);
            move |args| {
                let payload = args.first().and_then(Var::as_str).ok_or_else(|| {
                    ScriptError::Type("transform payload: expected string, found null".to_string())
                })?;
                let value = crate::script::json_to_var(payload)?;
                *lock(&handle) = transform_from_var(&value)?;
                Ok(Var::Null)
            }
        })?;

        // the compiled shims — one `typeof` guard per hook, hook names read
        // through a host fn so `HookNames` stays data. (The `__v12_*` host
        // fns are JSON-marshalling wrappers: they hand back REAL JS values,
        // so the get side needs no JSON.parse; the set side passes the
        // `transform` object's JSON text, which the wrapper carries as a
        // string for the host to decode.)
        self.script.register_internal("__v12_hook_names", {
            let hooks = self.hooks.clone();
            move |_args| {
                Ok(Var::List(crate::List::from_iter([
                    Var::from(hooks.initialize.as_str()),
                    Var::from(hooks.update.as_str()),
                    Var::from(hooks.draw.as_str()),
                ])))
            }
        })?;
        self.script.add_bootstrap(
            "var __v12_hooks = __v12_hook_names(); \
             function __v12_initialize() { \
                 globalThis.transform = __v12_transform_get(); \
                 if (typeof globalThis[__v12_hooks[0]] === 'function') { globalThis[__v12_hooks[0]](); } \
                 __v12_transform_set(JSON.stringify(globalThis.transform)); \
             } \
             function __v12_frame(dt) { \
                 globalThis.transform = __v12_transform_get(); \
                 if (typeof globalThis[__v12_hooks[1]] === 'function') { globalThis[__v12_hooks[1]](dt); } \
                 __v12_transform_set(JSON.stringify(globalThis.transform)); \
             } \
             function __v12_draw() { \
                 globalThis.transform = __v12_transform_get(); \
                 if (typeof globalThis[__v12_hooks[2]] === 'function') { globalThis[__v12_hooks[2]](); } \
                 __v12_transform_set(JSON.stringify(globalThis.transform)); \
             }",
        )
    }

    /// Drive one lifecycle hook through the scalar fast path, failing loud
    /// on a script error (an uncaught throw reaching the pump; a Rust
    /// `Component` has no error channel).
    fn drive(&mut self, shim: &str, hook: &str, dt: f64) {
        if let Err(e) = self.script.call_unit(shim, &[dt]) {
            panic!("scripted component '{hook}' failed: {e}");
        }
    }
}

impl Component for ScriptedComponent {
    fn priority(&self) -> Priority {
        self.priority
    }

    fn initialize(&mut self) {
        self.drive("__v12_initialize", "initialize", 0.0);
    }

    fn update(&mut self, dt: f64) {
        self.drive("__v12_frame", "update", dt);
    }

    fn draw(&mut self) {
        self.drive("__v12_draw", "draw", 0.0);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Transform → the `transform` global's shape (`o!{}`-built, floats
/// widened to f64 the way every bridge value crosses).
fn transform_to_var(t: &Transform) -> Var {
    Var::from(crate::o! {
        "position": crate::o! {
            "x": f64::from(t.position.x),
            "y": f64::from(t.position.y),
            "z": f64::from(t.position.z),
        },
        "rotation": crate::o! {
            "x": f64::from(t.rotation.x),
            "y": f64::from(t.rotation.y),
            "z": f64::from(t.rotation.z),
            "w": f64::from(t.rotation.w),
        },
        "scale": crate::o! {
            "x": f64::from(t.scale.x),
            "y": f64::from(t.scale.y),
            "z": f64::from(t.scale.z),
        },
    })
}

/// A float read at a dotted path — the [`FromVarError`](crate::FromVarError)
/// voice (`"transform.position.x": expected float, found null`).
fn f32_at(value: &Var, path: &str) -> Result<f32, ScriptError> {
    let found = value.get_path(path);
    found.as_f64().map(|f| f as f32).ok_or_else(|| {
        ScriptError::Type(format!(
            "transform.{path}: expected float, found {}",
            found.ty()
        ))
    })
}

/// The `transform` global's shape → Transform. Strict: a missing or
/// mis-typed component is an error (explicit reads; no silent defaults).
fn transform_from_var(value: &Var) -> Result<Transform, ScriptError> {
    if !matches!(value, Var::Object(_)) {
        return Err(ScriptError::Type(format!(
            "transform: expected object, found {}",
            value.ty()
        )));
    }
    Ok(Transform {
        position: Vec3::new(
            f32_at(value, "position.x")?,
            f32_at(value, "position.y")?,
            f32_at(value, "position.z")?,
        ),
        rotation: Quat {
            x: f32_at(value, "rotation.x")?,
            y: f32_at(value, "rotation.y")?,
            z: f32_at(value, "rotation.z")?,
            w: f32_at(value, "rotation.w")?,
        },
        scale: Vec3::new(
            f32_at(value, "scale.x")?,
            f32_at(value, "scale.y")?,
            f32_at(value, "scale.z")?,
        ),
    })
}
