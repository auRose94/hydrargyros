//! The interpreter seam — a `script` facade whose first engine is **v12**
//! (DevYatsu's JavaScript engine, behind the `script` feature), loaded
//! through the [`dynamic`](crate::dynamic) module's script-engine registry.
//!
//! The facade is "a configurable, embeddable script": the source loads
//! (at construction or via [`Script::load`]), then `run()`,
//! `set_global`/`get_global`, `call(name, args)` — and host and script
//! share values through the bridge. [`Script`] is that surface, and
//! `Var` is the bridge between native Rust and scripts. The bridge
//! itself (JSON over host-function calls, fidelity checks, the reserved
//! `__v12_` plumbing) lives in [`crate::dynamic::v12`] — the engine
//! backend — so other engines can swap in behind the same registry
//! without this facade changing.
//!
//! # The thread-local pool (why the handle is Send + Sync)
//!
//! Script engines like v12 are `!Send + !Sync` (single-mutator model:
//! state is created, driven, and dropped on ONE thread), while the `Var`
//! value graph is `Send + Sync` and `pub trait Component: Send`
//! (`crate::game::component`) means a scripted component must be `Send` —
//! and the winit backend runs the pump on its own thread. So a `Script` is
//! a lightweight **handle** (id + source + registered host closures as
//! `Send` closures) and the `!Send` session lives in a **thread-local
//! pool keyed by script id**, materialized lazily on first use on the
//! calling thread. The pump is the single mutator, so all calls for one
//! script happen on one thread and state persists per (script, thread):
//! moving a `Script` to another thread materializes a fresh session there
//! and re-runs the source (fresh script state, same host closures).
//!
//! A session must never live in a component field — it would make the
//! component `!Send` and break the trait's contract.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::dynamic::engine::{create_script_session, validate_user_name, HostFn, ScriptSession};
use crate::var::Var;

pub use crate::dynamic::engine::ScriptError;
pub(crate) use crate::dynamic::engine::{json_to_var, validate_identifier};

/// Process-global script-id counter (the pool's key). Relaxed is enough —
/// ids only need to be distinct, never ordered.
static SCRIPT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// The registry name of the engine this facade drives by default.
const DEFAULT_SCRIPT_ENGINE: &str = "v12";

// The handle crosses threads (components, the winit pump thread); the
// session it stands for does not — that is the whole design.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Script>();
    assert_send_sync::<ScriptError>();
};

/// Run `f` with the script's materialized session for THIS thread,
/// materializing it on first use. The entry is *taken out* of the pool for
/// the duration so the pool's `RefCell` is never held across script
/// execution: a host closure may legally call into other scripts on this
/// thread while we run. A panicking host closure loses the entry (the next
/// use rematerializes — fresh script state, fail loud) and the re-entrancy
/// guard unwinds cleanly.
///
/// Re-entrancy: calling back into the SAME script while it is running is
/// refused ([`ScriptError::Type`]) — it would materialize a second session
/// and fork the script's state. The guard is panic-safe (popped on drop).
fn with_entry<T>(
    script: &Script,
    f: impl FnOnce(&mut Box<dyn ScriptSession>) -> Result<T, ScriptError>,
) -> Result<T, ScriptError> {
    if IN_FLIGHT.with(|st| st.borrow().contains(&script.id)) {
        return Err(ScriptError::type_err(format!(
            "re-entrant script call: script {} is already running on this thread \
             (a host closure called back into its own script)",
            script.id
        )));
    }
    let _guard = ReentrancyGuard::push(script.id);
    POOL.with(|pool| {
        let mut entry = match pool.borrow_mut().remove(&script.id) {
            Some(entry) => entry,
            None => materialize(script)?,
        };
        let out = f(&mut entry);
        pool.borrow_mut().insert(script.id, entry);
        out
    })
}

/// Materialize a fresh session for `script` on the calling thread — the
/// registry's create path for the default engine (the bridge plumbing,
/// host closures, the source, and the bootstraps all go live inside).
fn materialize(script: &Script) -> Result<Box<dyn ScriptSession>, ScriptError> {
    create_script_session(
        DEFAULT_SCRIPT_ENGINE,
        &script.source,
        &script.hosts,
        &script.bootstraps,
    )
}

thread_local! {
    /// Script id → materialized session, per thread. Entries are created
    /// lazily on first use and dropped when the thread exits (bounded by
    /// the number of scripts a thread touches — the shape of an engine
    /// process).
    static POOL: RefCell<HashMap<u64, Box<dyn ScriptSession>>> = RefCell::new(HashMap::new());
    /// Ids of scripts with an operation in flight on this thread (the
    /// re-entrancy guard, panic-safe through [`ReentrancyGuard`]).
    static IN_FLIGHT: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

/// Pushes `id` onto the in-flight stack, pops it on drop — a panicking host
/// closure must not wedge the guard for the rest of the thread's life.
struct ReentrancyGuard;
impl ReentrancyGuard {
    fn push(id: u64) -> ReentrancyGuard {
        IN_FLIGHT.with(|st| st.borrow_mut().push(id));
        ReentrancyGuard
    }
}
impl Drop for ReentrancyGuard {
    fn drop(&mut self) {
        IN_FLIGHT.with(|st| {
            st.borrow_mut().pop();
        });
    }
}

/// A configurable, embeddable script whose bridge shares values with the
/// host. A `Script` is a
/// lightweight `Send + Sync` HANDLE — the `!Send` engine session lives in
/// the thread-local pool (see the module docs). Methods take `&mut self`:
/// the single-mutator model means a script call mutates the session.
///
/// ```
/// use hyg::script::{Script, ScriptError};
/// use hyg::Var;
///
/// # fn main() -> Result<(), ScriptError> {
/// let mut script = Script::new("function greet(who) { return 'hi ' + who; }")?;
/// let greeting = script.call("greet", &[Var::from("bob")])?;
/// assert_eq!(greeting.as_str(), Some("hi bob"));
/// # Ok(())
/// # }
/// ```
pub struct Script {
    id: u64,
    source: String,
    /// Registered host closures (Send), carried on the handle so each thread
    /// that materializes the script re-binds them into its own session.
    hosts: Vec<(String, HostFn)>,
    /// Extra source evaluated after the script source on every
    /// materialization (the scripted-component hook shims arm these).
    bootstraps: Vec<String>,
}

impl fmt::Debug for Script {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hosts: Vec<&str> = self.hosts.iter().map(|(n, _)| n.as_str()).collect();
        f.debug_struct("Script")
            .field("id", &self.id)
            .field("source_len", &self.source.len())
            .field("hosts", &hosts)
            .finish()
    }
}

impl Script {
    /// A script from its source. The
    /// source is evaluated immediately on the calling thread (v12 has no
    /// compile/run split, so parse errors surface here, at
    /// construction, as [`ScriptError::Engine`]).
    ///
    /// # Errors
    /// Compile errors, or a throw from top-level script code.
    pub fn new(source: impl Into<String>) -> Result<Script, ScriptError> {
        let mut script = Script {
            id: SCRIPT_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
            source: source.into(),
            hosts: Vec::new(),
            bootstraps: Vec::new(),
        };
        script.rematerialize()?;
        Ok(script)
    }

    /// The pool key (useful when debugging the per-thread pool).
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Replace the source and reset the script — parse + compile fresh.
    /// Script state starts over; registered host closures and bootstraps
    /// are re-bound.
    ///
    /// # Errors
    /// Compile errors, or a throw from top-level script code.
    pub fn load(&mut self, source: impl Into<String>) -> Result<(), ScriptError> {
        self.source = source.into();
        self.rematerialize()
    }

    /// (Re-)execute the loaded source. v12
    /// has no compile/run split (and its completion values are unwired,
    /// ADR-004), so this returns `Null`: scripts deliver values through
    /// [`call`](Self::call). Top-level `let`/`const` redeclare resets that
    /// state — REPL semantics.
    ///
    /// # Errors
    /// Compile errors, or a throw from top-level script code.
    pub fn run(&mut self) -> Result<Var, ScriptError> {
        with_entry(self, |session| session.run())
    }

    /// Set a global — fallible on purpose:
    /// non-JSON-representable values (non-finite floats, integers beyond
    /// 2^53) are rejected instead of silently degraded.
    ///
    /// A `Var::Func` becomes a callable in the script — the host and
    /// script sharing one value space — via the same host-fn path as
    /// [`register`](Self::register).
    ///
    /// # Errors
    /// Invalid/reserved name, unrepresentable value, engine error.
    pub fn set_global(&mut self, name: &str, value: &Var) -> Result<(), ScriptError> {
        validate_user_name(name)?;
        if let Var::Func(func) = value {
            // a host function value: registered as a callable global
            let host: HostFn = Arc::new(Mutex::new(Box::new({
                let func = func.clone();
                move |args: &[Var]| Ok(func.call(args))
            })));
            self.push_host(name, host);
            return Ok(());
        }
        with_entry(self, |session| session.set_global(name, value))
    }

    /// Read a global. A missing global is
    /// an error (explicit reads; no silent nulls), and a function value is
    /// an error (functions cannot cross out of scripts).
    ///
    /// Reads resolve **global-object** state: `var` declarations, function
    /// declarations, `globalThis.x = …` writes, and everything
    /// [`set_global`](Self::set_global) wrote. Top-level `let`/`const` are
    /// v12-lexical (each eval's lexical scope is its own — see
    /// [`run`](Self::run)): the script's own functions see them across calls
    /// (same-eval closures), but a later eval — this probe — cannot. That is
    /// a v12 divergence from spec; surface such state through a script
    /// accessor instead. A
    /// global holding `undefined` reads as not defined (JS `null` reads as
    /// [`Var::Null`]).
    ///
    /// # Errors
    /// Missing global, function value, engine error.
    pub fn get_global(&mut self, name: &str) -> Result<Var, ScriptError> {
        validate_identifier(name)?;
        with_entry(self, |session| session.get_global(name))
    }

    /// Call a global function with `Var` args. Values cross through the
    /// JSON bridge; a
    /// missing function is a [`ScriptError::Type`], a script throw is a
    /// [`ScriptError::Engine`]. Resolution is global-object state (see
    /// [`get_global`](Self::get_global)): declare hooks with `function` so
    /// they resolve.
    ///
    /// # Errors
    /// Missing function, unrepresentable args, thrown script error.
    pub fn call(&mut self, name: &str, args: &[Var]) -> Result<Var, ScriptError> {
        validate_identifier(name)?;
        with_entry(self, |session| session.call(name, args))
    }

    /// The per-frame fast path: call `name` with scalar `f64` args and
    /// discard the result, straight through the engine — no JSON
    /// round-trip, no recompile. This is the scripted-component pump shape
    /// (lifecycle hooks return nothing).
    ///
    /// # Errors
    /// Missing function (the hook shims guard with `typeof`, so the
    /// component layer never hits this), thrown script error.
    pub fn call_unit(&mut self, name: &str, args: &[f64]) -> Result<(), ScriptError> {
        validate_identifier(name)?;
        with_entry(self, |session| session.call_unit(name, args))
    }

    /// Register a Rust closure as a callable global in the script — the
    /// host-function seam (the engine's host-fn path, adapted through the
    /// same `Var` conversion: args arrive as owned `Var`s detached from any
    /// guard, the result crosses back through the bridge).
    ///
    /// Registering over an existing name replaces it. Panics on an invalid
    /// or reserved name (programmer error, like `Var::set` on a non-object);
    /// a host closure that panics fails the whole call loudly and the
    /// script's session rematerializes fresh on the next use.
    ///
    /// The closure must be `Send` — it is carried on the handle and bound
    /// into whichever thread's session materializes next.
    pub fn register(&mut self, name: &str, mut f: impl FnMut(&[Var]) -> Var + Send + 'static) {
        if let Err(e) = validate_user_name(name) {
            panic!("{e}");
        }
        let host: HostFn = Arc::new(Mutex::new(Box::new(move |args: &[Var]| Ok(f(args)))));
        self.push_host(name, host);
    }

    /// The bridge's own host fns (reserved `__v12_` names allowed), with the
    /// `Result` return so plumbing can throw into JS. `pub(crate)`: the
    /// component layer arms its transform seam through this.
    pub(crate) fn register_internal(
        &mut self,
        name: &str,
        f: impl FnMut(&[Var]) -> Result<Var, ScriptError> + Send + 'static,
    ) -> Result<(), ScriptError> {
        validate_identifier(name)?;
        let host: HostFn = Arc::new(Mutex::new(Box::new(f)));
        self.push_host(name, host);
        Ok(())
    }

    /// Extra source evaluated after the script source on every
    /// materialization — the hook shims the component layer arms. Evaluated
    /// immediately too (on the current thread's session) so arm-time errors
    /// surface here.
    pub(crate) fn add_bootstrap(&mut self, js: impl Into<String>) -> Result<(), ScriptError> {
        let js = js.into();
        with_entry(self, |session| session.eval_bootstrap(&js))?;
        self.bootstraps.push(js);
        Ok(())
    }

    /// Push a host onto the handle and bind it into the current thread's
    /// session when one exists (it does right after `new`; it does not while
    /// the script is mid-call — the entry is checked out of the pool for
    /// re-entrancy safety — or on a thread that never materialized the
    /// script). In those cases the binding is deferred: the handle keeps the
    /// closure and the next materialization binds it, which means a script
    /// cannot see a host registered by its own running closure until it is
    /// reloaded.
    fn push_host(&mut self, name: &str, host: HostFn) {
        self.hosts.retain(|(n, _)| n != name);
        self.hosts.push((name.to_string(), Arc::clone(&host)));
        POOL.with(|pool| {
            let mut pool = pool.borrow_mut();
            if let Some(entry) = pool.get_mut(&self.id) {
                if let Err(e) = entry.bind_host(name, Arc::clone(&host)) {
                    // Names are pre-validated, so a binding failure is a
                    // bridge bug — fail loud rather than silently drift.
                    panic!("script bridge: cannot bind host fn '{name}': {e}");
                }
            }
        });
    }

    /// Drop this thread's session (if any) and materialize a fresh one —
    /// parse + compile from scratch.
    fn rematerialize(&mut self) -> Result<(), ScriptError> {
        POOL.with(|pool| {
            pool.borrow_mut().remove(&self.id);
        });
        let session = materialize(self)?;
        POOL.with(|pool| {
            pool.borrow_mut().insert(self.id, session);
        });
        Ok(())
    }
}
