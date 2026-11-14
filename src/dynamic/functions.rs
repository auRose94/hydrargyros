//! The callable-function table — the one registry that plugins, script
//! functions, and (at the graph milestone) function nodes all register
//! into. Three faces of one table: every callable is a [`HostFn`] closure
//! over `Var` args plus a [`FunctionMeta`] descriptor that editors,
//! inspectors, and the graph tier read.
//!
//! Pure data, no feature gate — like [`crate::dynamic::engine`]. The table
//! is a process-global `RwLock` map; registering over an existing name
//! replaces it (the upgrade path), and unknown names are explicit errors
//! that name what IS registered. Entries outlive the handles that made
//! them (a callable keeps its own state alive); a plugin's entries go when
//! [`replace_plugin_functions`] retires them with an empty set.
//!
//! The registry namespaces are enforced by construction, not by policy:
//! callers build their own names, and the plugin loader only ever writes
//! `"{plugin_id}."` keys — a plugin cannot reach another plugin's or the
//! host's callables.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use crate::dynamic::engine::{HostFn, ScriptError};
use crate::var::{Ty, Var};

/// The descriptor behind one callable — what an editor shows for a node
/// and what the graph tier's pin checking reads. Pins are `Var`'s own
/// type vocabulary; an empty [`inputs`](FunctionMeta::inputs) means the
/// callable declared no signature (arity is then the callee's business).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionMeta {
    /// One line for editors/inspector (empty if unwritten).
    pub summary: String,
    /// Input pins in call order — arity is checked against this when set.
    pub inputs: Vec<Ty>,
    /// The output pin.
    pub output: Ty,
    /// Who registered this callable — `plugin:{id}:{version}`, `host`,
    /// `script:{id}` … a label for humans and the inspector, not a
    /// machine contract.
    pub origin: String,
}

struct FunctionEntry {
    meta: FunctionMeta,
    host: HostFn,
}

fn table() -> &'static RwLock<HashMap<String, FunctionEntry>> {
    static TABLE: OnceLock<RwLock<HashMap<String, FunctionEntry>>> = OnceLock::new();
    TABLE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register (or replace) a callable. Insertion over an existing name
/// replaces it — re-registering upgrades, nothing else.
pub fn register_function(name: impl Into<String>, meta: FunctionMeta, host: HostFn) {
    table()
        .write()
        .expect("function table")
        .insert(name.into(), FunctionEntry { meta, host });
}

/// Replace every callable in `plugin_id`'s namespace (`"{plugin_id}."`
/// keys) with `entries`, under ONE write guard — the atomic swap the
/// hot-swap contract needs: concurrent callers hit either all-old or
/// all-new per namespace, never a mix. Returns the number of entries
/// retired. Calling with an empty `entries` is the unregister path.
pub fn replace_plugin_functions(
    plugin_id: &str,
    entries: Vec<(String, FunctionMeta, HostFn)>,
) -> usize {
    let prefix = format!("{plugin_id}.");
    let mut t = table().write().expect("function table");
    let retired: Vec<String> = t
        .keys()
        .filter(|name| name.starts_with(&prefix))
        .cloned()
        .collect();
    let count = retired.len();
    for name in retired {
        t.remove(&name);
    }
    for (name, meta, host) in entries {
        debug_assert!(
            name.starts_with(&prefix),
            "plugin entries must stay inside the plugin's namespace"
        );
        t.insert(name, FunctionEntry { meta, host });
    }
    count
}

/// The registered callable names (sorted — introspection/debug output).
#[must_use]
pub fn function_names() -> Vec<String> {
    let mut names: Vec<String> = table()
        .read()
        .expect("function table")
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

/// The descriptor behind a callable (a clone — the table moves on).
#[must_use]
pub fn function_meta(name: &str) -> Option<FunctionMeta> {
    table()
        .read()
        .expect("function table")
        .get(name)
        .map(|e| e.meta.clone())
}

/// Call a registered callable with `Var` args. An unknown name is an
/// explicit [`ScriptError::Type`] naming what IS registered; a declared
/// signature with the wrong arity fails before the callable runs.
///
/// # Errors
/// Unknown name, arity mismatch against the declared pins, marshal
/// failure, or the callable's own error (a script throw or a plugin's
/// [`ScriptError::Engine`]).
pub fn call_function(name: &str, args: &[Var]) -> Result<Var, ScriptError> {
    // The guard's scope ends BEFORE the call and before the error path:
    // the error re-reads the table for the registered-names report, and
    // re-reading while this thread still holds the read guard deadlocks
    // the moment a parallel writer queues (the registry's day-one bug).
    let entry = {
        let guard = table().read().expect("function table");
        guard.get(name).map(|e| (e.meta.clone(), Arc::clone(&e.host)))
    };
    let Some((meta, host)) = entry else {
        return Err(ScriptError::type_err(format!(
            "unknown function {name:?} (registered: {:?})",
            function_names(),
        )));
    };
    if !meta.inputs.is_empty() && meta.inputs.len() != args.len() {
        return Err(ScriptError::type_err(format!(
            "{name}: expected {} args (pins {:?}), got {}",
            meta.inputs.len(),
            meta.inputs,
            args.len(),
        )));
    }
    // The host closure's mutex is NOT the table lock — a slow callable
    // (script throw, plugin sleep) never blocks registration or lookup.
    let mut host = host.lock().expect("function host");
    host(args)
}
