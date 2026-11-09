//! The script-engine registry — the seam that makes script BACKENDS
//! pluggable. A [`ScriptEngine`] is a factory for [`ScriptSession`]s: one
//! materialized script on one thread. The v12 JavaScript engine registers
//! as `"v12"` (behind the `script` feature); future engines (another JS, a
//! Lua, a WASM runtime) register the same way and no call site changes.
//!
//! Pure data, no feature gate — like the web siblings. The registry is a
//! process-global `RwLock` map (the audio-backend pattern): built-ins are
//! pre-seeded on first access, `register_script_engine` inserts over
//! existing names (re-registering replaces), and unknown names are
//! explicit errors, never silent fallbacks.
//!
//! # The Send line
//!
//! A [`ScriptEngine`] is `Send + Sync` (any thread may create sessions);
//! a [`ScriptSession`] is deliberately **NOT `Send`** — engines like v12
//! are single-mutator (`!Send + !Sync`) and a session is owned by the one
//! thread that materialized it (the caller keeps it in thread-local
//! storage; see `crate::script`). Handles that cross threads carry the
//! *inputs* (source, host closures, bootstraps) and materialize a fresh
//! session per thread — that contract lives above this seam.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::var::Var;

/// Reserved prefix for the v12 bridge's own plumbing (`__v12_args`,
/// `__v12_response`, …). User names that start with it are rejected so
/// scripts can't shadow the machinery. (The name is legacy-branded from
/// the first engine; the *contract* — a reserved namespace — is what
/// matters, and every engine must keep it.)
pub const RESERVED_PREFIX: &str = "__v12_";

/// A registered host closure, shared so the handle keeps it while each
/// materialized session binds a clone of the `Arc`. Poison-tolerant through
/// [`crate::lock`]: a panicking host closure must not brick the script.
pub type HostFn = Arc<Mutex<Box<dyn FnMut(&[Var]) -> Result<Var, ScriptError> + Send>>>;

/// Failure of a script operation — the engine's error rendered
/// ([`ScriptError::Engine`]) or the bridge/contract's own conversion
/// failures with path-style messages (`"transform.position.x": expected
/// float, found null` — the [`FromVarError`](crate::FromVarError) voice)
/// as [`ScriptError::Type`].
#[derive(Debug)]
pub enum ScriptError {
    /// The engine refused or the script threw (rendered engine error).
    Engine(String),
    /// A conversion or contract failure on the bridge itself.
    Type(String),
}

impl ScriptError {
    pub fn type_err(msg: impl Into<String>) -> ScriptError {
        ScriptError::Type(msg.into())
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScriptError::Engine(msg) | ScriptError::Type(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for ScriptError {}

/// One materialized script on ONE thread. Deliberately `!Send` (see the
/// module docs): state persists per (script, thread), and every method
/// takes `&mut self` — engines are single-mutator.
pub trait ScriptSession {
    /// (Re-)execute the loaded source. Engines without a compile/run split
    /// re-run; completion values are engine-specific, so the contract is
    /// "values deliver through [`call`](Self::call)".
    ///
    /// # Errors
    /// Compile errors, or a throw from top-level script code.
    fn run(&mut self) -> Result<Var, ScriptError>;

    /// Set a global (a plain value; function values are host bindings made
    /// above this seam). Fallible on purpose: unrepresentable values are
    /// rejected, never silently degraded.
    ///
    /// # Errors
    /// Invalid/reserved name, unrepresentable value, engine error.
    fn set_global(&mut self, name: &str, value: &Var) -> Result<(), ScriptError>;

    /// Read a global. Missing global and function value are explicit
    /// errors (no silent nulls, no pretending to call).
    ///
    /// # Errors
    /// Missing global, function value, engine error.
    fn get_global(&mut self, name: &str) -> Result<Var, ScriptError>;

    /// Call a global function with `Var` args. A missing function is a
    /// [`ScriptError::Type`], a script throw an [`ScriptError::Engine`].
    ///
    /// # Errors
    /// Missing function, unrepresentable args, thrown script error.
    fn call(&mut self, name: &str, args: &[Var]) -> Result<Var, ScriptError>;

    /// The per-frame fast path: scalar `f64` args, result discarded, no
    /// marshalling overhead. The scripted-component pump shape.
    ///
    /// # Errors
    /// Missing function, thrown script error.
    fn call_unit(&mut self, name: &str, args: &[f64]) -> Result<(), ScriptError>;

    /// Bind a host closure as a callable global. Re-binding a name
    /// replaces it (the registry's own semantics, mirrored here so a
    /// host can be re-registered mid-session).
    ///
    /// # Errors
    /// Invalid/reserved name, engine error.
    fn bind_host(&mut self, name: &str, host: HostFn) -> Result<(), ScriptError>;

    /// Evaluate extra source AFTER the main source on every
    /// materialization — the hook shims the component layer arms.
    ///
    /// # Errors
    /// Compile errors, or a throw from the bootstrap code.
    fn eval_bootstrap(&mut self, js: &str) -> Result<(), ScriptError>;
}

/// A script engine: a named factory for sessions. State-free by
/// convention — all per-script state lives in the session it builds.
pub trait ScriptEngine: Send + Sync {
    /// The registry name (`"v12"` for the built-in).
    fn name(&self) -> &str;

    /// Materialize a fresh session for THIS thread: the source evaluates,
    /// host closures bind, bootstraps run — all session state is live when
    /// this returns, so construction errors (parse errors, top-level
    /// throws) surface here.
    ///
    /// # Errors
    /// Compile errors, a throw from top-level code, or a binding failure.
    fn session(
        &self,
        source: &str,
        hosts: &[(String, HostFn)],
        bootstraps: &[String],
    ) -> Result<Box<dyn ScriptSession>, ScriptError>;
}

/// The engine map, pre-seeded with the built-ins on first access.
fn engines() -> &'static RwLock<HashMap<String, Arc<dyn ScriptEngine>>> {
    static ENGINES: OnceLock<RwLock<HashMap<String, Arc<dyn ScriptEngine>>>> = OnceLock::new();
    ENGINES.get_or_init(|| {
        let mut map: HashMap<String, Arc<dyn ScriptEngine>> = HashMap::new();
        #[cfg(feature = "script")]
        {
            map.insert("v12".to_string(), Arc::new(crate::dynamic::v12::V12Engine));
        }
        RwLock::new(map)
    })
}

/// Register (or replace) a script engine under `name`. Insertion over an
/// existing name replaces it — re-registering upgrades, nothing else.
pub fn register_script_engine(name: impl Into<String>, engine: Arc<dyn ScriptEngine>) {
    engines().write().expect("script engine registry").insert(name.into(), engine);
}

/// The registered engine names (debug/introspection).
#[must_use]
pub fn script_engine_names() -> Vec<String> {
    let mut names: Vec<String> = engines()
        .read()
        .expect("script engine registry")
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

/// Materialize a session for `engine_name` — the registry's create path.
/// An unknown name is an explicit [`ScriptError::Type`], never a silent
/// fallback.
///
/// # Errors
/// Unknown engine name, or the engine's own session failures.
pub fn create_script_session(
    engine_name: &str,
    source: &str,
    hosts: &[(String, HostFn)],
    bootstraps: &[String],
) -> Result<Box<dyn ScriptSession>, ScriptError> {
    // The guard's scope ends BEFORE the error path: the closure re-reads
    // the registry for the registered-names report, and re-reading while
    // this thread still holds the read guard deadlocks the moment a
    // parallel writer queues (the pending writer blocks the second read).
    let engine = {
        let guard = engines().read().expect("script engine registry");
        guard.get(engine_name).cloned()
    };
    let engine = engine.ok_or_else(|| {
        ScriptError::type_err(format!(
            "unknown script engine {engine_name:?} (registered: {:?})",
            script_engine_names(),
        ))
    })?;
    engine.session(source, hosts, bootstraps)
}

/// ASCII identifier check for global names (engines interpolate them into
/// generated sources and global-table keys; the component layer validates
/// hook names with it too).
pub(crate) fn validate_identifier(name: &str) -> Result<(), ScriptError> {
    let mut chars = name.chars();
    let ok = match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(ScriptError::type_err(format!(
            "'{name}' is not a valid script global name (ASCII identifier expected)"
        )))
    }
}

/// Reject user names in the bridge's reserved namespace.
pub(crate) fn validate_user_name(name: &str) -> Result<(), ScriptError> {
    validate_identifier(name)?;
    if name.starts_with(RESERVED_PREFIX) {
        return Err(ScriptError::type_err(format!(
            "'{name}' starts with the reserved '{RESERVED_PREFIX}' prefix (the bridge's own plumbing)"
        )));
    }
    Ok(())
}

/// JSON text → `Var` (the bridge's decode path). `{"__func__": id}`
/// restores a registered `Func` through the serde contract, so a function
/// handle that crossed in can cross back out. Parsed from the TEXT (not a
/// `serde_json::Value` tree) so object key order survives.
pub(crate) fn json_to_var(json: &str) -> Result<Var, ScriptError> {
    serde_json::from_str(json).map_err(|e| ScriptError::type_err(format!("script value: {e}")))
}
