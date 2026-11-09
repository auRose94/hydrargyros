//! The interpreter seam — a `script` facade driven by the **v12**
//! JavaScript engine (DevYatsu/v12, a git dependency — the roadmap's
//! "DevYatsu's script crate" name was wrong; the crate is `v12-api`).
//!
//! The facade is "a configurable, embeddable script": the source loads
//! (at construction or via [`Script::load`]), then `run()`,
//! `set_global`/`get_global`, `call(name, args)` — and host and script
//! share values through the bridge. [`Script`] is that surface, and
//! `Var` is the bridge between native Rust and scripts.
//!
//! # The thread-local pool (why the handle is Send + Sync)
//!
//! v12's `Context` is `!Send + !Sync` (single-mutator model: it must be
//! created, driven, and dropped on ONE thread), while the `Var` value graph
//! is `Send + Sync` and `pub trait Component: Send`
//! (`crate::game::component`) means a scripted component must be `Send` —
//! and the winit backend runs the pump on its own thread. So a `Script` is
//! a lightweight **handle** (id + source + registered host closures as
//! `Send` closures) and the `!Send` `Context` lives in a **thread-local
//! pool keyed by script id**, materialized lazily on first use on the
//! calling thread. The pump is the single mutator, so all calls for one
//! script happen on one thread and state persists per (script, thread):
//! moving a `Script` to another thread materializes a fresh context there
//! and re-runs the source (fresh script state, same host closures).
//!
//! A `Context` must never live in a component field — it would make the
//! component `!Send` and break the trait's contract.
//!
//! # The bridge (explicit, both directions)
//!
//! v12's public surface marshals only `bool`/`i32`/`f64`/`String`/`&str`/`()`
//! (there is no high-level object API and — as of the pinned rev —
//! expression-statement completion values are `undefined` per v12's ADR-004
//! gap, so values can NEVER be read back through `eval`). The bridge is
//! therefore JSON text over host-function calls, explicit at every hop:
//!
//! - **Scalars and structures** cross as JSON: `serde_json` on one side
//!   (`Var` has full serde impls), `JSON.parse`/`JSON.stringify` on the
//!   other. `Var::Func` rides the serde contract (`{"__func__": id}`), so a
//!   function handle that crossed in can cross back out and be restored.
//!   Fidelity is checked, not assumed: an `I64` beyond 2^53 or a non-finite
//!   `F64` is a [`ScriptError::Type`] on the way in — JS numbers are f64 and
//!   silently degrading them would be the exact implicit-conversion footgun
//!   this crate exists to avoid. JS `null`/`undefined` read back as
//!   [`Var::Null`]; integral JSON numbers read back as `I64` (JS has one
//!   number type, so `F64(5.0)` normalizes to `I64(5)` across the bridge).
//!   A script *function value* crossing OUT is an error — we don't pretend
//!   to call it.
//! - **The one scalar fast path** is [`Script::call_unit`]: an `f64`-arg,
//!   discard-result call straight through v12's `ToValue` — no JSON, no
//!   recompile. That is the per-frame shape (the scripted-component pump),
//!   where payloads stay scalar by design: JSON round-trips are O(size) per
//!   call, so keep crossed payloads small.
//!
//! Host functions registered with [`Script::register`] become real JS
//! globals (a JSON-marshalling wrapper around a v12 host fn); a `Var::Func`
//! crossing IN through `set_global` becomes one the same way — scripts
//! and host read and write each other through that shared surface.
//!
//! One v12-fidelity note the facade makes honest: each eval's top-level
//! `let`/`const` bindings are **lexical to that eval** (a v12 divergence
//! from spec). They
//! persist for the script's own functions (same-eval closures keep state
//! across calls — the update-loop shape), but the bridge's later evals
//! cannot see them, so `get_global`/`call` resolve **global-object** state:
//! `var`, function declarations, `globalThis.x = …`, and set_global writes.
//! Surface lexical state through a script accessor.
//!
//! # Re-entrancy and locks
//!
//! Host closures run *inside* a script call. All facade calls go through a
//! frame stack (see [`BridgeState`]) so a host closure may legally call into
//! ANOTHER script on the same thread; calling back into the SAME script
//! while it is running is refused ([`ScriptError::Type`]) — it would
//! materialize a second context and fork the script's state. Two rules keep
//! locks deadlock-free (the `Var` payload mutexes are plain `Mutex`es):
//! never hold a `Var`'s guard across a script call that could re-enter a
//! host closure touching the same `Var`, and never lock in the opposite
//! order (`bridge` then payload vs payload then `bridge`). The bridge holds
//! no payload guard across any JS execution: values are detached (JSON
//! text) before the interpreter runs.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use v12_api::{Context, FromValue, JsValue, ToValue, V12Error};

use crate::list::List;
use crate::lock;
use crate::var::Var;

/// JS numbers are f64; integers beyond 2^53 cannot cross the bridge exactly.
/// (serde_json would carry them as text and JS would silently round — the
/// explicit check turns that into an error instead.)
const MAX_SAFE_INTEGER: i64 = 9007199254740992; // 2^53

/// Reserved prefix for the bridge's own plumbing (`__v12_args`,
/// `__v12_response`, the per-component frame bootstrap, …). User names that
/// start with it are rejected so scripts can't shadow the machinery.
const BRIDGE_PREFIX: &str = "__v12_";

/// Process-global script-id counter (the pool's key). Relaxed is enough —
/// ids only need to be distinct, never ordered.
static SCRIPT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// A registered host closure, shared so the handle keeps it while each
/// materialized context binds a clone of the `Arc` (v12's `register_fn`
/// needs a `'static` closure and the handle must keep re-binding it on
/// whichever thread materializes next). Poison-tolerant through
/// [`crate::lock`]: a panicking host closure must not brick the script.
type HostFn = Arc<Mutex<Box<dyn FnMut(&[Var]) -> Result<Var, ScriptError> + Send>>>;

/// Failure of a script operation — v12's error wrapped, plus the bridge's
/// own conversion failures with path-style messages
/// (`"transform.position.x": expected float, found null` — the
/// [`FromVarError`](crate::FromVarError) voice).
#[derive(Debug)]
pub enum ScriptError {
    /// The engine refused or the script threw (v12's `V12Error`).
    Engine(V12Error),
    /// A conversion or contract failure on the bridge itself.
    Type(String),
}

impl ScriptError {
    fn type_err(msg: impl Into<String>) -> ScriptError {
        ScriptError::Type(msg.into())
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScriptError::Engine(e) => write!(f, "{e}"),
            ScriptError::Type(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for ScriptError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ScriptError::Engine(e) => Some(e),
            ScriptError::Type(_) => None,
        }
    }
}

impl From<V12Error> for ScriptError {
    fn from(e: V12Error) -> Self {
        ScriptError::Engine(e)
    }
}

// The handle crosses threads (components, the winit pump thread); the
// context it stands for does not — that is the whole design.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Script>();
    assert_send_sync::<ScriptError>();
};

/// One frame of an in-flight facade operation. Stacked so re-entrant calls
/// (a host closure calling another script) each see their own top frame:
/// `__v12_args` takes the top frame's pending args, `__v12_response` writes
/// the top frame's response, and the facade pops exactly the frame it pushed.
struct Frame {
    args_json: Option<String>,
    response: Option<String>,
}

/// The host→script protocol state, one per materialized context. Shared with
/// the bridge's host closures (`Arc<Mutex>`); no `Var` payload guard is ever
/// held while it is locked — the values that cross are detached JSON text.
struct BridgeState {
    frames: Vec<Frame>,
}

/// A materialized context plus its protocol state. `Context` is `!Send`, so
/// a `PoolEntry` is too — it only ever lives in the thread-local pool.
struct PoolEntry {
    ctx: Context,
    bridge: Arc<Mutex<BridgeState>>,
}

thread_local! {
    /// Script id → materialized context, per thread. Entries are created
    /// lazily on first use and dropped when the thread exits (bounded by the
    /// number of scripts a thread touches — the shape of an engine process).
    static POOL: RefCell<HashMap<u64, PoolEntry>> = RefCell::new(HashMap::new());
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

/// Run `f` with the script's materialized context for THIS thread,
/// materializing it on first use. The entry is *taken out* of the pool for
/// the duration so the pool's `RefCell` is never held across script
/// execution: a host closure may legally call into other scripts on this
/// thread while we run. A panicking host closure loses the entry (the next
/// use rematerializes — fresh script state, fail loud) and the re-entrancy
/// guard unwinds cleanly.
fn with_entry<T>(
    script: &Script,
    f: impl FnOnce(&mut PoolEntry) -> Result<T, ScriptError>,
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

/// Materialize a fresh context for `script` on the calling thread: bridge
/// plumbing, every registered host closure, then the source (v12 has no
/// compile/run split, so "compile" happens with execution, once per
/// (script, thread)), then the
/// bootstraps (hook shims that must see the script's own declarations).
fn materialize(script: &Script) -> Result<PoolEntry, ScriptError> {
    let mut ctx = Context::new();
    let bridge = Arc::new(Mutex::new(BridgeState { frames: Vec::new() }));

    // the bridge's own plumbing — reserved `__v12_` names
    register_bridge_args(&mut ctx, &bridge)?;
    register_bridge_response(&mut ctx, &bridge)?;

    // host closures the handle carries (re-bound on every materialization,
    // so the closures follow the script across threads)
    for (name, host) in &script.hosts {
        bind_host_fn(&mut ctx, name, host)?;
    }

    ctx.eval::<()>(&script.source)?;
    for bootstrap in &script.bootstraps {
        ctx.eval::<()>(bootstrap)?;
    }

    Ok(PoolEntry { ctx, bridge })
}

/// `__v12_args()` — hands the top frame's pending JSON args to JS (the
/// facade stashed them before driving the eval).
fn register_bridge_args(
    ctx: &mut Context,
    bridge: &Arc<Mutex<BridgeState>>,
) -> Result<(), ScriptError> {
    let bridge = Arc::clone(bridge);
    ctx.register_fn("__v12_args", move |heap, _this, _args| {
        let json = lock(&bridge)
            .frames
            .last_mut()
            .and_then(|frame| frame.args_json.take())
            .unwrap_or_else(|| "[]".to_string());
        Ok(json.to_value(heap))
    })
    .map_err(ScriptError::from)
}

/// `__v12_response(text)` — JS delivering a response string back to the
/// facade (`'0:'` missing, `'f:'` function value, `'1:' + JSON payload`).
fn register_bridge_response(
    ctx: &mut Context,
    bridge: &Arc<Mutex<BridgeState>>,
) -> Result<(), ScriptError> {
    let bridge = Arc::clone(bridge);
    ctx.register_fn("__v12_response", move |heap, _this, args| {
        let text = <String as FromValue>::from_value(
            &*heap,
            args.first().copied().unwrap_or_else(JsValue::undefined),
        )
        .ok_or_else(|| "bridge protocol violation: response is not a string".to_value(heap))?;
        if let Some(frame) = lock(&bridge).frames.last_mut() {
            frame.response = Some(text);
        }
        Ok(JsValue::undefined())
    })
    .map_err(ScriptError::from)
}

/// Register `name` as a real JS global on `ctx`: a v12 host fn
/// `__v12_user_<name>` that decodes a JSON args array, runs the closure, and
/// returns the result as JSON — wrapped by a JS shim so the script calls
/// `name(...)` with real values. Registering over an existing name replaces
/// it (v12's own semantics).
///
/// (v12's `Heap` type is not nameable from here — the facade re-exports the
/// conversion traits, not the heap — which is exactly why object values
/// marshal as JSON strings and why the wrapper shim exists at all.)
fn bind_host_fn(ctx: &mut Context, name: &str, host: &HostFn) -> Result<(), ScriptError> {
    let host_fn_name = format!("{BRIDGE_PREFIX}user_{name}");
    let host = Arc::clone(host);
    ctx.register_fn(&host_fn_name, move |heap, _this, args| {
        let json = <String as FromValue>::from_value(
            &*heap,
            args.first().copied().unwrap_or_else(JsValue::undefined),
        )
        .ok_or_else(|| "bridge protocol violation: host args are not a string".to_value(heap))?;
        let vars = decode_arg_array(&json).map_err(|e| e.to_string().to_value(heap))?;
        // The args are detached (JSON text → owned Vars) before the closure
        // runs: never hold a Var payload guard across user code that could
        // re-enter the bridge.
        let out = lock(&host)(&vars).map_err(|e| e.to_string().to_value(heap))?;
        let json_out = var_to_json(&out).map_err(|e| e.to_string().to_value(heap))?;
        Ok(json_out.to_value(heap))
    })
    .map_err(ScriptError::from)?;

    // the JS shim: real scalars/objects in and out, JSON on the wire
    let install = format!(
        "globalThis[{name_json}] = function(...__a) {{ return JSON.parse({host_fn_name}(JSON.stringify(__a))); }};",
        name_json = json_text(name),
    );
    ctx.eval::<()>(&install)?;
    Ok(())
}

/// A Rust string as a JSON/JS string literal (`"he said \"hi\""` — JSON
/// string syntax is valid JS string syntax).
fn json_text(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// Decode a JSON array text into owned `Var`s (a host fn's argument list) —
/// parsed from the text so the script's object key order survives.
fn decode_arg_array(json: &str) -> Result<Vec<Var>, ScriptError> {
    serde_json::from_str::<Vec<Var>>(json)
        .map_err(|e| ScriptError::type_err(format!("host function arguments: {e}")))
}

/// `Var` → JSON text, with the bridge's fidelity checks applied (integers
/// beyond [`MAX_SAFE_INTEGER`], non-finite floats — both become explicit
/// errors instead of silent degradation). Serialized straight from the
/// `Var` (not through `serde_json::Value`, whose map sorts keys — `Var`
/// objects are insertion-ordered and the bridge keeps them that way).
fn var_to_json(v: &Var) -> Result<String, ScriptError> {
    check_fidelity(v)?;
    serde_json::to_string(v)
        .map_err(|e| ScriptError::type_err(format!("value cannot cross into the script: {e}")))
}

/// Walk a `Var` tree enforcing what JS can hold exactly.
fn check_fidelity(v: &Var) -> Result<(), ScriptError> {
    match v {
        // conservative, saturation-proof bound: |i| ≤ 2^53 is exactly what a
        // JS double holds for integers (a float round-trip check would be
        // fooled — `as` saturates instead of rounding)
        Var::I64(i) => {
            if i.unsigned_abs() > MAX_SAFE_INTEGER as u64 {
                return Err(ScriptError::type_err(format!(
                    "integer {i} is not exactly representable as a JS number (beyond 2^53)"
                )));
            }
            Ok(())
        }
        Var::F64(f) => {
            if f.is_finite() {
                Ok(())
            } else {
                Err(ScriptError::type_err(format!(
                    "float {f} is not finite — JS numbers cannot hold it"
                )))
            }
        }
        Var::List(l) => l.iter().iter().try_for_each(check_fidelity),
        Var::Object(o) => o
            .entries()
            .iter()
            .map(|(_, v)| v)
            .try_for_each(check_fidelity),
        _ => Ok(()),
    }
}

/// JSON text → `Var`. `{"__func__": id}` restores a registered `Func`
/// through the serde contract, so a function handle that crossed in can
/// cross back out. Parsed from the TEXT (not a `serde_json::Value` tree)
/// so object key order survives. (`pub(crate)`: the component layer decodes
/// bridged payloads through the same path.)
pub(crate) fn json_to_var(json: &str) -> Result<Var, ScriptError> {
    serde_json::from_str(json).map_err(|e| ScriptError::type_err(format!("script value: {e}")))
}

/// Decode a bridge response string: `'0:'` missing, `'f:'` a function value
/// (an error — we don't pretend to call it), `'1:' + JSON payload` a value
/// (`"undefined"` payload → `Null`).
fn decode_response(response: &str, what: &str) -> Result<Var, ScriptError> {
    let (code, payload) = response
        .split_once(':')
        .ok_or_else(|| ScriptError::type_err("bridge protocol violation: malformed response"))?;
    match code {
        "0" => Err(ScriptError::type_err(format!("{what} is not defined"))),
        "f" => Err(ScriptError::type_err(format!(
            "{what} is a script function value — functions cannot cross out of scripts; \
             call it in the script (or host it with Script::register)"
        ))),
        "1" => match payload {
            "undefined" => Ok(Var::Null),
            _ => json_to_var(payload),
        },
        _ => Err(ScriptError::type_err(
            "bridge protocol violation: unknown response code",
        )),
    }
}

/// ASCII identifier check for global names (the bridge interpolates them
/// into JS sources and `globalThis[...]` keys; the component layer validates
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
fn validate_user_name(name: &str) -> Result<(), ScriptError> {
    validate_identifier(name)?;
    if name.starts_with(BRIDGE_PREFIX) {
        return Err(ScriptError::type_err(format!(
            "'{name}' starts with the reserved '{BRIDGE_PREFIX}' prefix (the bridge's own plumbing)"
        )));
    }
    Ok(())
}

/// Panic-on-misuse variant of name validation for infallible signatures.
fn expect_name(name: &str) {
    if let Err(e) = validate_user_name(name) {
        panic!("{e}");
    }
}

/// Push a protocol frame (the facade side of the bridge).
fn push_frame(entry: &mut PoolEntry, args_json: Option<String>) {
    lock(&entry.bridge).frames.push(Frame {
        args_json,
        response: None,
    });
}

/// Drive an eval that only needs arg delivery (no response expected) —
/// `set_global`. The frame is popped before the eval error is mapped so a
/// throw surfaces as the script's error, not as a protocol violation.
fn eval_with_args(
    entry: &mut PoolEntry,
    args_json: Option<String>,
    source: &str,
) -> Result<(), ScriptError> {
    push_frame(entry, args_json);
    let out = entry.ctx.eval::<()>(source);
    lock(&entry.bridge).frames.pop();
    out.map_err(ScriptError::from)
}

/// A configurable, embeddable script whose bridge shares values with the
/// host. A `Script` is a
/// lightweight `Send + Sync` HANDLE — the `!Send` v12 `Context` lives in the
/// thread-local pool (see the module docs). Methods take `&mut self`: the
/// single-mutator model means a script call mutates the context.
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
    /// that materializes the script re-binds them into its own context.
    hosts: Vec<(String, HostFn)>,
    /// Extra JS evaluated after the source on every materialization (the
    /// scripted-component hook shims arm these).
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
        with_entry(self, |entry| {
            entry.ctx.eval::<()>(&self.source)?;
            Ok(Var::Null)
        })
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
        let json = var_to_json(value)?;
        let install = format!(
            "globalThis[{name_json}] = JSON.parse({BRIDGE_PREFIX}args());",
            name_json = json_text(name),
        );
        with_entry(self, |entry| eval_with_args(entry, Some(json), &install))
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
        let getter = format!("{BRIDGE_PREFIX}get");
        let install = format!(
            "globalThis[{getter_json}] = function () {{ \
                 var v = globalThis[{name_json}]; \
                 if (typeof v === 'undefined') return '0:'; \
                 if (typeof v === 'function') return 'f:'; \
                 return '1:' + JSON.stringify(v); }};",
            getter_json = json_text(&getter),
            name_json = json_text(name),
        );
        let response = with_entry(self, |entry| {
            entry.ctx.eval::<()>(&install).map_err(ScriptError::from)?;
            entry
                .ctx
                .call::<String, ()>(&getter, &[])
                .map_err(ScriptError::from)
        })?;
        decode_response(&response, &format!("global '{name}'"))
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
        // the whole arg list is one JSON array (a List's serde shape)
        let args_json = var_to_json(&Var::List(List::from_iter(args.iter().cloned())))?;
        let driver = format!("{BRIDGE_PREFIX}call");
        let install = format!(
            "globalThis[{driver_json}] = function () {{ \
                 var f = globalThis[{name_json}]; \
                 if (typeof f !== 'function') return '0:'; \
                 var r = f.apply(null, JSON.parse({BRIDGE_PREFIX}args())); \
                 return (typeof r === 'function') ? 'f:' : ('1:' + JSON.stringify(r)); }};",
            driver_json = json_text(&driver),
            name_json = json_text(name),
        );
        let response = with_entry(self, |entry| {
            entry.ctx.eval::<()>(&install).map_err(ScriptError::from)?;
            push_frame(entry, Some(args_json));
            let out = entry.ctx.call::<String, ()>(&driver, &[]);
            lock(&entry.bridge).frames.pop();
            out.map_err(ScriptError::from)
        })?;
        decode_response(&response, &format!("script function '{name}'"))
    }

    /// The per-frame fast path: call `name` with scalar `f64` args and
    /// discard the result, straight through v12's `ToValue` — no JSON
    /// round-trip, no recompile. This is the scripted-component pump shape
    /// (lifecycle hooks return nothing).
    ///
    /// # Errors
    /// Missing function (the hook shims guard with `typeof`, so the
    /// component layer never hits this), thrown script error.
    pub fn call_unit(&mut self, name: &str, args: &[f64]) -> Result<(), ScriptError> {
        validate_identifier(name)?;
        with_entry(self, |entry| {
            entry
                .ctx
                .call::<(), f64>(name, args)
                .map_err(ScriptError::from)
        })
    }

    /// Register a Rust closure as a callable global in the script — the
    /// host-function seam (v12's `register_fn`, adapted through the same
    /// `Var` conversion path: args arrive as owned `Var`s detached from any
    /// guard, the result crosses back through the JSON bridge).
    ///
    /// Registering over an existing name replaces it. Panics on an invalid
    /// or reserved name (programmer error, like `Var::set` on a non-object);
    /// a host closure that panics fails the whole call loudly and the
    /// script's context rematerializes fresh on the next use.
    ///
    /// The closure must be `Send` — it is carried on the handle and bound
    /// into whichever thread's context materializes next.
    pub fn register(&mut self, name: &str, mut f: impl FnMut(&[Var]) -> Var + Send + 'static) {
        expect_name(name);
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

    /// Extra JS evaluated after the source on every materialization — the
    /// hook shims the component layer arms. Evaluated immediately too (on
    /// the current thread's context) so arm-time errors surface here.
    pub(crate) fn add_bootstrap(&mut self, js: impl Into<String>) -> Result<(), ScriptError> {
        let js = js.into();
        with_entry(self, |entry| {
            entry.ctx.eval::<()>(&js).map_err(ScriptError::from)
        })?;
        self.bootstraps.push(js);
        Ok(())
    }

    /// Push a host onto the handle and bind it into the current thread's
    /// context when one exists (it does right after `new`; it does not while
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
                if let Err(e) = bind_host_fn(&mut entry.ctx, name, &host) {
                    // Names are pre-validated, so a binding failure is a
                    // bridge bug — fail loud rather than silently drift.
                    panic!("script bridge: cannot bind host fn '{name}': {e}");
                }
            }
        });
    }

    /// Drop this thread's context (if any) and materialize a fresh one —
    /// parse + compile from scratch.
    fn rematerialize(&mut self) -> Result<(), ScriptError> {
        POOL.with(|pool| {
            pool.borrow_mut().remove(&self.id);
        });
        let entry = materialize(self)?;
        POOL.with(|pool| {
            pool.borrow_mut().insert(self.id, entry);
        });
        Ok(())
    }
}
