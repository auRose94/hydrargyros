//! The hyg FFI-plugin contract — everything a native plugin is built
//! against, shared by the host ([`hyg::dynamic::ffi`], behind the
//! `dynamic-ffi` feature) and by every plugin author (unconditionally).
//!
//! A plugin is a shared library exporting two mandated symbols (plain
//! `#[no_mangle] extern "C"`, no macro machinery):
//!
//! ```rust,ignore
//! /// The plugin manifest — a `.json.hyg` Var document as JSON text.
//! #[no_mangle]
//! pub extern "C" fn hyg_plugin_manifest() -> RString;
//!
//! /// Dispatch a call by name: one JSON text per arg in, one JSON text out.
//! #[no_mangle]
//! pub extern "C" fn hyg_plugin_call(
//!     name: RString,
//!     args: RVec<RString>,
//! ) -> RResult<RString, RBoxError>;
//! ```
//!
//! The values crossing the boundary are abi_stable's layout-stable types
//! (`RString`/`RVec`/`RBoxError`) — Rust-shaped signatures, not a raw C
//! pointer protocol (docs/dynamic-design.md). Data crosses as JSON text —
//! the same wire the script bridge speaks — so a plugin function, a script
//! function, and a graph function node are three faces of one function
//! table on the host side.
//!
//! The loader is deliberately NOT abi_stable's `RootModule` machinery:
//! `RootModule::load_from` caches the module in process-global statics
//! (one load per process) and leaks the library by design — both
//! incompatible with hyg's hot-swap contract, where a new version loads
//! side-by-side and the old one unloads after its in-flight calls drain.
//! The host loads libraries through `libloading` (the same wheel
//! abi_stable itself uses) and keeps the layout-stability guarantee where
//! it matters: in the data types crossing the boundary.
//!
//! # Rules for plugin authors
//!
//! Three rules keep a plugin loadable and unloadable sound:
//!
//! 1. **Use the default system allocator.** `RString`/`RVec` values are
//!    allocated on one side and dropped on the other; that is sound when
//!    both sides allocate through the system allocator (malloc) — replacing
//!    the global allocator in a plugin breaks the boundary.
//! 2. **Never unwind across the boundary.** A panic escaping an
//!    `extern "C"` fn aborts the process. `hyg_plugin_call` implementations
//!    wrap their dispatch in [`std::panic::catch_unwind`] and report the
//!    panic as an error ([`refuse`]).
//! 3. **No `thread_local!` state.** After a hot-swap the old library is
//!    unloaded once its in-flight calls finish; TLS destructors registered
//!    by an unloaded library on threads that outlive it are undefined
//!    behavior. Plugins keep per-call state in their host, not in TLS.
//!
//! The manifest ([`PluginManifest`]) is the machine-readable half of the
//! contract: id, version, ABI version, the functions offered (name, pins),
//! and the permissions the plugin wants. [`DYNAMIC_ABI`] is bumped whenever
//! the export signatures or the manifest schema change incompatibly; the
//! host refuses a manifest whose ABI differs from its own, before anything
//! registers.

use abi_stable::std_types::{RBoxError, ROk, RErr, RResult, RString, RVec};
use serde::{Deserialize, Serialize};

/// The plugin-protocol version this contract speaks. The host checks the
/// manifest's `abi` field against this before registering anything — a
/// mismatch is an explicit error naming both sides, never a silent load.
///
/// Bump on any breaking change to the mandated exports' signatures or the
/// manifest schema ([`PluginManifest`]).
pub const DYNAMIC_ABI: u64 = 1;

/// The manifest — a `.json.hyg` Var document describing the plugin. Built
/// by the plugin (serialize with [`PluginManifest::to_json`]), validated by
/// the host against [`DYNAMIC_ABI`] and the identifier/namespace rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    /// The plugin's namespace: functions register as `"{id}.{name}"` on the
    /// host. An ASCII identifier (the host validates it).
    pub id: String,
    /// The plugin's own version string — the hot-swap story: two builds of
    /// one plugin differ here, and the host reports which version retired.
    pub version: String,
    /// The protocol version this build speaks ([`DYNAMIC_ABI`]).
    pub abi: u64,
    /// The functions the plugin offers — each becomes a host callable.
    pub functions: Vec<FunctionSpec>,
    /// The permissions the plugin wants. M1's only host-enforceable seam is
    /// function registration, and it is enforced by construction: a plugin's
    /// functions register into its own `"{id}."` namespace and nothing else.
    /// The field rides along so richer host APIs (config access, asset
    /// paths, the agentic policy gates) can enforce against it later.
    #[serde(default)]
    pub permissions: Vec<String>,
}

impl PluginManifest {
    /// The manifest as JSON text (the `.json.hyg` Var document riding the
    /// mandated `hyg_plugin_manifest` export).
    ///
    /// # Panics
    /// The manifest's own serialization cannot fail (no maps with
    /// non-string keys, no floats) — a failure is a bug in this contract,
    /// so it panics.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("PluginManifest serializes by construction")
    }

    /// Parse the manifest text (the host's decode path).
    ///
    /// # Errors
    /// Malformed JSON or a missing required field.
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("plugin manifest: {e}"))
    }
}

/// One function a plugin offers: a name (unique within the plugin's
/// namespace), a one-line summary for editors and inspectors, and the pin
/// signature — input pins in call order, one output pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionSpec {
    /// The function's name within the plugin's namespace (an ASCII
    /// identifier; the host validates it).
    pub name: String,
    /// One line for editors/inspector (empty if unwritten).
    #[serde(default)]
    pub summary: String,
    /// Input pins, in call order. The host checks arity against this.
    pub inputs: Vec<PinTy>,
    /// The output pin.
    pub output: PinTy,
}

/// A data pin's `Var` type — the value system the function table speaks,
/// mirrored here because a plugin cannot depend on hyg itself. The host
/// maps these onto `Var::Ty` when it registers the function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PinTy {
    /// `Var::Null`
    Null,
    /// `Var::Bool`
    Bool,
    /// `Var::I64`
    I64,
    /// `Var::F64`
    F64,
    /// `Var::Str`
    Str,
    /// `Var::List`
    List,
    /// `Var::Object`
    Object,
    /// `Var::Func` (a function handle — `{"__func__": id}`)
    Func,
}

/// Parse one call argument: JSON text → a typed value. The standard first
/// line of a plugin function body.
///
/// # Errors
/// The index is out of range, or the JSON does not fit `T` (the error names
/// the argument position).
pub fn arg<T: serde::de::DeserializeOwned>(args: &RVec<RString>, index: usize) -> Result<T, String> {
    let text = args
        .get(index)
        .map(RString::as_str)
        .ok_or_else(|| format!("arg {index}: missing (got {} args)", args.len()))?;
    serde_json::from_str(text).map_err(|e| format!("arg {index}: {e}"))
}

/// Build the success side of `hyg_plugin_call`: a serde value → its JSON
/// text as the call's result.
///
/// # Errors
/// `value` cannot serialize as JSON.
pub fn respond<T: Serialize>(value: &T) -> RResult<RString, RBoxError> {
    match serde_json::to_string(value) {
        Ok(text) => ROk(RString::from(text)),
        Err(e) => refuse(format!("result serialization failed: {e}")),
    }
}

/// Build the error side of `hyg_plugin_call`: a human message the host
/// surfaces as the call's error. The message is formatted eagerly — nothing
/// borrows from the caller across the boundary.
pub fn refuse(msg: impl Into<String>) -> RResult<RString, RBoxError> {
    RErr(RBoxError::from_fmt(&msg.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_json_round_trips_with_pin_names() {
        let manifest = PluginManifest {
            id: "greeter".into(),
            version: "0.1.0".into(),
            abi: DYNAMIC_ABI,
            permissions: vec!["register:own".into()],
            functions: vec![FunctionSpec {
                name: "greet".into(),
                summary: "says hello".into(),
                inputs: vec![PinTy::Str],
                output: PinTy::Str,
            }],
        };
        let text = manifest.to_json();
        // the pin vocabulary is lowercase on the wire — the graph editor's
        // pin labels read exactly this
        assert!(text.contains(r#""inputs":["str"]"#), "{text}");
        let back = PluginManifest::from_json(&text).expect("round trip");
        assert_eq!(back, manifest);
    }

    #[test]
    fn args_parse_and_refuse() {
        let args = RVec::from(vec![RString::from("42"), RString::from("\"hi\"")]);
        let n: i64 = arg(&args, 0).expect("int parses");
        assert_eq!(n, 42);
        let s: String = arg(&args, 1).expect("str parses");
        assert_eq!(s, "hi");
        assert!(arg::<i64>(&args, 1).is_err(), "type mismatch is an error");
        assert!(arg::<i64>(&args, 9).is_err(), "out of range is an error");
    }
}
