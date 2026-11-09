//! The v12 backend — the first [`ScriptEngine`](crate::dynamic::engine::ScriptEngine),
//! registered as `"v12"`. This module owns the JSON bridge: everything
//! that makes v12's raw surface (`eval` scalars only, unwired completion
//! values per v12's ADR-004, `!Send` single-mutator contexts) speak the
//! `Var`-in/`Var`-out session contract.
//!
//! # The bridge (explicit, both directions)
//!
//! v12's public surface marshals only `bool`/`i32`/`f64`/`String`/`&str`/`()`
//! (no high-level object API). The bridge is therefore JSON text over
//! host-function calls, explicit at every hop:
//!
//! - **Scalars and structures** cross as JSON: `serde_json` on one side
//!   (`Var` has full serde impls), `JSON.parse`/`JSON.stringify` on the
//!   other. `Var::Func` rides the serde contract (`{"__func__": id}`), so a
//!   function handle that crossed in can cross back out and be restored.
//!   Fidelity is checked, not assumed: an `I64` beyond 2^53 or a non-finite
//!   `F64` is a [`ScriptError::Type`](crate::dynamic::engine::ScriptError::Type)
//!   on the way in — JS numbers are f64 and silently degrading them would be
//!   the exact implicit-conversion footgun this crate exists to avoid. JS
//!   `null`/`undefined` read back as [`Var::Null`]; integral JSON numbers
//!   read back as `I64` (JS has one number type, so `F64(5.0)` normalizes
//!   to `I64(5)` across the bridge). A script *function value* crossing OUT
//!   is an error — we don't pretend to call it.
//! - **The one scalar fast path** is
//!   [`call_unit`](crate::dynamic::engine::ScriptSession::call_unit): an
//!   `f64`-arg, discard-result call straight through v12's `ToValue` — no
//!   JSON, no recompile. That is the per-frame shape (the
//!   scripted-component pump), where payloads stay scalar by design: JSON
//!   round-trips are O(size) per call, so keep crossed payloads small.
//!
//! Host functions bound with
//! [`bind_host`](crate::dynamic::engine::ScriptSession::bind_host) become
//! real JS globals (a JSON-marshalling wrapper around a v12 host fn); a
//! `Var::Func` crossing IN through `set_global` becomes one the same way —
//! scripts and host read and write each other through that shared surface.
//!
//! One v12-fidelity note the bridge makes honest: each eval's top-level
//! `let`/`const` bindings are **lexical to that eval** (a v12 divergence
//! from spec). They persist for the script's own functions (same-eval
//! closures keep state across calls — the update-loop shape), but the
//! bridge's later evals cannot see them, so `get_global`/`call` resolve
//! **global-object** state: `var`, function declarations, `globalThis.x = …`,
//! and set_global writes. Surface lexical state through a script accessor.
//!
//! # Re-entrancy and locks
//!
//! Host closures run *inside* a script call. Every session operation goes
//! through a frame stack ([`BridgeState`]) so a host closure may legally
//! call into ANOTHER session on the same thread; calling back into the
//! SAME script while it is running is refused
//! ([`ScriptError::Type`](crate::dynamic::engine::ScriptError::Type)) by
//! the pool above — it would materialize a second context and fork the
//! script's state. Two rules keep locks deadlock-free (the `Var` payload
//! mutexes are plain `Mutex`es): never hold a `Var`'s guard across a
//! script call that could re-enter a host closure touching the same `Var`,
//! and never lock in the opposite order (`bridge` then payload vs payload
//! then `bridge`). The bridge holds no payload guard across any JS
//! execution: values are detached (JSON text) before the interpreter runs.

use std::sync::{Arc, Mutex};

use v12_api::{Context, FromValue, JsValue, ToValue, V12Error};

use crate::dynamic::engine::{
    json_to_var, HostFn, ScriptEngine, ScriptError, ScriptSession, RESERVED_PREFIX,
};
use crate::list::List;
use crate::lock;
use crate::var::Var;

/// JS numbers are f64; integers beyond 2^53 cannot cross the bridge exactly.
/// (serde_json would carry them as text and JS would silently round — the
/// explicit check turns that into an error instead.)
const MAX_SAFE_INTEGER: i64 = 9007199254740992; // 2^53

/// One frame of an in-flight bridge operation. Stacked so re-entrant calls
/// (a host closure calling another script) each see their own top frame:
/// the args host fn takes the top frame's pending args, the response host
/// fn writes the top frame's response, and the operation pops exactly the
/// frame it pushed.
struct Frame {
    args_json: Option<String>,
    response: Option<String>,
}

/// The host→script protocol state, one per materialized session. Shared
/// with the bridge's host closures (`Arc<Mutex>`); no `Var` payload guard
/// is ever held while it is locked — the values that cross are detached
/// JSON text.
struct BridgeState {
    frames: Vec<Frame>,
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

impl From<V12Error> for ScriptError {
    fn from(e: V12Error) -> Self {
        ScriptError::Engine(e.to_string())
    }
}

/// `__v12_args()` — hands the top frame's pending JSON args to JS (the
/// session stashed them before driving the eval).
fn register_bridge_args(
    ctx: &mut Context,
    bridge: &Arc<Mutex<BridgeState>>,
) -> Result<(), ScriptError> {
    let bridge = Arc::clone(bridge);
    ctx.register_fn(&format!("{RESERVED_PREFIX}args"), move |heap, _this, _args| {
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
/// bridge (`'0:'` missing, `'f:'` function value, `'1:'` + JSON payload).
fn register_bridge_response(
    ctx: &mut Context,
    bridge: &Arc<Mutex<BridgeState>>,
) -> Result<(), ScriptError> {
    let bridge = Arc::clone(bridge);
    ctx.register_fn(&format!("{RESERVED_PREFIX}response"), move |heap, _this, args| {
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
/// `__v12_user_<name>` that decodes a JSON args array, runs the closure,
/// and returns the result as JSON — wrapped by a JS shim so the script
/// calls `name(...)` with real values. Registering over an existing name
/// replaces it (v12's own semantics).
///
/// (v12's `Heap` type is not nameable from here — the engine re-exports
/// the conversion traits, not the heap — which is exactly why object values
/// marshal as JSON strings and why the wrapper shim exists at all.)
fn bind_host_fn(ctx: &mut Context, name: &str, host: &HostFn) -> Result<(), ScriptError> {
    let host_fn_name = format!("{RESERVED_PREFIX}user_{name}");
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

/// The v12 engine (the `"v12"` registry entry).
pub struct V12Engine;

impl ScriptEngine for V12Engine {
    fn name(&self) -> &str {
        "v12"
    }

    fn session(
        &self,
        source: &str,
        hosts: &[(String, HostFn)],
        bootstraps: &[String],
    ) -> Result<Box<dyn ScriptSession>, ScriptError> {
        Ok(Box::new(V12Session::new(source, hosts, bootstraps)?))
    }
}

/// One materialized v12 context plus its bridge protocol state. `!Send`
/// by construction (`Context` is single-mutator) — it only ever lives in
/// the thread-local pool above this seam.
struct V12Session {
    ctx: Context,
    bridge: Arc<Mutex<BridgeState>>,
    source: String,
}

impl V12Session {
    /// Materialize: bridge plumbing, every registered host closure, then
    /// the source (v12 has no compile/run split, so "compile" happens with
    /// execution, once per (script, thread)), then the bootstraps (hook
    /// shims that must see the script's own declarations).
    fn new(
        source: &str,
        hosts: &[(String, HostFn)],
        bootstraps: &[String],
    ) -> Result<V12Session, ScriptError> {
        let mut ctx = Context::new();
        let bridge = Arc::new(Mutex::new(BridgeState { frames: Vec::new() }));

        // the bridge's own plumbing — reserved names
        register_bridge_args(&mut ctx, &bridge)?;
        register_bridge_response(&mut ctx, &bridge)?;

        // host closures the handle carries (re-bound on every
        // materialization, so the closures follow the script across threads)
        for (name, host) in hosts {
            bind_host_fn(&mut ctx, name, host)?;
        }

        ctx.eval::<()>(source)?;
        for bootstrap in bootstraps {
            ctx.eval::<()>(bootstrap)?;
        }

        Ok(V12Session { ctx, bridge, source: source.to_string() })
    }

    /// Push a protocol frame (the caller's side of the bridge).
    fn push_frame(&mut self, args_json: Option<String>) {
        lock(&self.bridge).frames.push(Frame {
            args_json,
            response: None,
        });
    }

    /// Drive an eval that only needs arg delivery (no response expected) —
    /// `set_global`. The frame is popped before the eval error is mapped so
    /// a throw surfaces as the script's error, not as a protocol violation.
    fn eval_with_args(&mut self, args_json: Option<String>, source: &str) -> Result<(), ScriptError> {
        self.push_frame(args_json);
        let out = self.ctx.eval::<()>(source);
        lock(&self.bridge).frames.pop();
        out.map_err(ScriptError::from)
    }
}

impl ScriptSession for V12Session {
    fn run(&mut self) -> Result<Var, ScriptError> {
        self.ctx.eval::<()>(&self.source)?;
        Ok(Var::Null)
    }

    fn set_global(&mut self, name: &str, value: &Var) -> Result<(), ScriptError> {
        let json = var_to_json(value)?;
        let install = format!(
            "globalThis[{name_json}] = JSON.parse({RESERVED_PREFIX}args());",
            name_json = json_text(name),
        );
        self.eval_with_args(Some(json), &install)
    }

    fn get_global(&mut self, name: &str) -> Result<Var, ScriptError> {
        let getter = format!("{RESERVED_PREFIX}get");
        let install = format!(
            "globalThis[{getter_json}] = function () {{ \
                 var v = globalThis[{name_json}]; \
                 if (typeof v === 'undefined') return '0:'; \
                 if (typeof v === 'function') return 'f:'; \
                 return '1:' + JSON.stringify(v); }};",
            getter_json = json_text(&getter),
            name_json = json_text(name),
        );
        self.ctx.eval::<()>(&install).map_err(ScriptError::from)?;
        let response = self
            .ctx
            .call::<String, ()>(&getter, &[])
            .map_err(ScriptError::from)?;
        decode_response(&response, &format!("global '{name}'"))
    }

    fn call(&mut self, name: &str, args: &[Var]) -> Result<Var, ScriptError> {
        // the whole arg list is one JSON array (a List's serde shape)
        let args_json = var_to_json(&Var::List(List::from_iter(args.iter().cloned())))?;
        let driver = format!("{RESERVED_PREFIX}call");
        let install = format!(
            "globalThis[{driver_json}] = function () {{ \
                 var f = globalThis[{name_json}]; \
                 if (typeof f !== 'function') return '0:'; \
                 var r = f.apply(null, JSON.parse({RESERVED_PREFIX}args())); \
                 return (typeof r === 'function') ? 'f:' : ('1:' + JSON.stringify(r)); }};",
            driver_json = json_text(&driver),
            name_json = json_text(name),
        );
        self.ctx.eval::<()>(&install).map_err(ScriptError::from)?;
        self.push_frame(Some(args_json));
        // the driver function itself returns the response string
        let response = self.ctx.call::<String, ()>(&driver, &[]);
        lock(&self.bridge).frames.pop();
        let response = response.map_err(ScriptError::from)?;
        decode_response(&response, &format!("script function '{name}'"))
    }

    fn call_unit(&mut self, name: &str, args: &[f64]) -> Result<(), ScriptError> {
        self.ctx.call::<(), f64>(name, args).map_err(ScriptError::from)
    }

    fn bind_host(&mut self, name: &str, host: HostFn) -> Result<(), ScriptError> {
        bind_host_fn(&mut self.ctx, name, &host)
    }

    fn eval_bootstrap(&mut self, js: &str) -> Result<(), ScriptError> {
        self.ctx.eval::<()>(js).map_err(ScriptError::from)
    }
}
