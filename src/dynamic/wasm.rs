//! The WASM plugin tier — the **sandboxed** face of the plugin story
//! ([dynamic-design.md](../../docs/dynamic-design.md)). Native FFI (M1)
//! is the trusted tier: it can crash the process and sees memory
//! directly. A WASM plugin can do neither — wasmtime runs it with **no
//! ambient authority wired**: no WASI, so no filesystem, no network, no
//! clock, no host calls the host didn't bind. Anything a wasm plugin
//! does to the outside world crosses the host, and the host enforces
//! the manifest's `permissions` at the boundary before anything
//! registers.
//!
//! # The contract (the wasm mirror of the native plugin)
//!
//! A wasm plugin is a module exporting:
//!
//! ```text
//! memory                                      ; the linear memory
//! alloc(size: i32) -> i32                     ; guest heap bump allocator
//! hyg_manifest(out: i32, cap: i32) -> i32     ; writes the manifest JSON, returns len (-1: cap too small)
//! hyg_call(name: i32, name_len: i32,          ; the dispatcher (same shape
//!          args: i32, args_len: i32,          ;   as the native contract's
//!          out: i32, cap: i32) -> i32         ;   hyg_plugin_call), JSON in/out
//! ```
//!
//! The values crossing the boundary are the same as everywhere else in
//! hyg: JSON text over the Var bridge (the script bridge's wire), the
//! manifest is the SAME `.json.hyg` Var document the native contract
//! speaks, and functions register into the SAME function table —
//! `"{id}.{name}"`, graph-callable like everything else. The trust
//! difference is the HOST, not the data: no host imports are bound, so
//! the module's whole world is its own memory and the bytes the host
//! hands it.
//!
//! # Permission enforcement
//!
//! The host is loaded with a **capability grant** — the permissions it
//! is willing to offer (`load_wasm_plugin`'s `grants`). The manifest's
//! requested permissions must be a subset or the load refuses naming
//! both sides: nothing registers, nothing runs. (The one permission
//! every tier gets by construction is registration into its own
//! `"{id}."` namespace.) A richer grant story — a host API the
//! permissions actually gate — lands with the host-import surface;
//! the refusal shape is stable now so plugins can't be written against
//! a fiction.
//!
//! Feature-gated behind `dynamic-wasm` (wasmtime is a heavy dep; the
//! core stays lean).

use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use wasmtime::{AsContextMut, Engine, Linker, Module, Store};

use crate::dynamic::engine::{json_to_var, validate_identifier, HostFn, ScriptError};
use crate::dynamic::functions::{replace_plugin_functions, FunctionMeta};
// the contract, re-exported so host callers speak one vocabulary
use crate::var::{Ty, Var};
pub use hyg_plugin_api::{FunctionSpec, PinTy, PluginManifest, DYNAMIC_ABI};

/// The process-global wasmtime engine — one compiler cache for every
/// sandboxed plugin (modules compile once per engine).
fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(Engine::default)
}

/// Failure of a sandboxed plugin load or call — every variant names the
/// boundary that refused.
#[derive(Debug)]
pub enum WasmError {
    /// The file could not be read, or the module failed to compile or
    /// instantiate (the wasmtime message included).
    Load(String),
    /// A mandated export was missing or mistyped.
    Export(String),
    /// The manifest text is not a valid [`PluginManifest`] document.
    Manifest(String),
    /// The manifest's `abi` is not this host's [`DYNAMIC_ABI`].
    Abi { expected: u64, found: u64 },
    /// The manifest requests permissions the host's grant does not
    /// include — nothing registers, nothing runs.
    Permission {
        requested: Vec<String>,
        granted: Vec<String>,
    },
    /// A call into the module failed (export signature, capacity, or a
    /// guest-side refusal).
    Call(String),
    /// A name/identifier contract failure.
    Name(String),
}

impl fmt::Display for WasmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WasmError::Load(msg) => write!(f, "wasm plugin load: {msg}"),
            WasmError::Export(msg) => write!(f, "wasm plugin export: {msg}"),
            WasmError::Manifest(msg) => write!(f, "wasm plugin manifest: {msg}"),
            WasmError::Abi { expected, found } => write!(
                f,
                "wasm plugin ABI mismatch: host expects {expected}, manifest reports {found} \
                 — rebuild the plugin against this hyg-plugin-api"
            ),
            WasmError::Permission { requested, granted } => write!(
                f,
                "wasm plugin permissions exceed the host grant: requested {requested:?}, granted {granted:?} \
                 — the sandbox does not grant what it was not given"
            ),
            WasmError::Call(msg) => write!(f, "wasm plugin call: {msg}"),
            WasmError::Name(msg) => write!(f, "wasm plugin name: {msg}"),
        }
    }
}

impl std::error::Error for WasmError {}

/// The response-buffer capacity for manifest and call round trips.
const OUT_CAP: usize = 64 * 1024;

/// A loaded, validated, sandboxed plugin. Calls serialize through the
/// instance's store (wasm execution is single-threaded per store).
pub struct WasmPlugin {
    /// The module, pre-compiled into the shared engine (kept for
    /// introspection; re-instantiation is the wasm hot-swap path).
    #[allow(dead_code)] // never read — kept alive with the instance
    module: Module,
    instance: Mutex<StoreAndInstance>,
    /// The validated manifest (owned — rebuilt from the exported JSON).
    manifest: PluginManifest,
    /// The manifest as the `.json.hyg` Var it is.
    manifest_var: Var,
    /// The capability grant the host loaded this plugin under.
    granted: Vec<String>,
}

struct StoreAndInstance {
    store: Store<()>,
    instance: wasmtime::Instance,
}

impl WasmPlugin {
    /// The plugin's namespace id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    /// The validated manifest.
    #[must_use]
    pub fn manifest(&self) -> PluginManifest {
        self.manifest.clone()
    }

    /// The manifest as the `.json.hyg` Var it is.
    #[must_use]
    pub fn manifest_var(&self) -> Var {
        self.manifest_var.clone()
    }

    /// The capability grant this plugin loaded under.
    #[must_use]
    pub fn granted(&self) -> &[String] {
        &self.granted
    }

    /// Call one of the module's functions: `Var` args → one JSON array
    /// text → the guest's dispatcher → JSON text → `Var`. The same wire
    /// the script bridge speaks.
    ///
    /// # Errors
    /// [`WasmError::Call`] for transport failures (the guest returns -1
    /// on its own refusal) — a function's ERROR result arrives as a
    /// successful call whose JSON is the plugin's error shape, or as
    /// this error when the transport itself fails.
    pub fn call(&self, name: &str, args: &[Var]) -> Result<Var, WasmError> {
        let args_json =
            serde_json::to_string(args).map_err(|e| WasmError::Call(format!("args: {e}")))?;
        let mut guard = self.instance.lock().expect("wasm plugin instance");
        let StoreAndInstance { store, instance } = &mut *guard;

        // guest buffers: name + args, both guest-allocated (the module's
        // own heap — the host never writes outside what alloc returned)
        let name_bytes = name.as_bytes();
        let name_ptr = self.guest_alloc(store, instance, name_bytes.len())?;
        write_guest(store, instance, name_ptr, name_bytes)?;
        let args_bytes = args_json.as_bytes();
        let args_ptr = self.guest_alloc(store, instance, args_bytes.len())?;
        write_guest(store, instance, args_ptr, args_bytes)?;

        let out_ptr = self.guest_alloc(store, instance, OUT_CAP)?;
        let call = instance
            .get_typed_func::<(i32, i32, i32, i32, i32, i32), i32>(
                store.as_context_mut(),
                "hyg_call",
            )
            .map_err(|e| WasmError::Export(format!("hyg_call: {e}")))?;
        let written = call
            .call(
                store.as_context_mut(),
                (
                    name_ptr as i32,
                    name_bytes.len() as i32,
                    args_ptr as i32,
                    args_bytes.len() as i32,
                    out_ptr as i32,
                    OUT_CAP as i32,
                ),
            )
            .map_err(|e| WasmError::Call(format!("hyg_call trapped: {e}")))?;
        if written < 0 {
            return Err(WasmError::Call(format!(
                "{name}: the module refused (capacity or contract violation)"
            )));
        }
        let written = written as usize;
        if written > OUT_CAP {
            return Err(WasmError::Call(format!(
                "{name}: response length lies ({written})"
            )));
        }
        let mut out = vec![0u8; written];
        instance
            .get_memory(store.as_context_mut(), "memory")
            .ok_or_else(|| WasmError::Export("memory".to_string()))?
            .read(store.as_context_mut(), out_ptr, &mut out)
            .map_err(|e| WasmError::Call(format!("read response: {e}")))?;
        let text = String::from_utf8(out)
            .map_err(|e| WasmError::Call(format!("response is not UTF-8: {e}")))?;
        json_to_var(&text).map_err(|e| WasmError::Call(format!("{name}: result: {e}")))
    }

    fn guest_alloc(
        &self,
        store: &mut Store<()>,
        instance: &wasmtime::Instance,
        size: usize,
    ) -> Result<usize, WasmError> {
        let alloc = instance
            .get_typed_func::<i32, i32>(store.as_context_mut(), "alloc")
            .map_err(|e| WasmError::Export(format!("alloc: {e}")))?;
        let ptr = alloc
            .call(store.as_context_mut(), size as i32)
            .map_err(|e| WasmError::Call(format!("alloc trapped: {e}")))?;
        if ptr <= 0 {
            return Err(WasmError::Call(format!(
                "alloc returned {ptr} for {size} bytes"
            )));
        }
        Ok(ptr as usize)
    }
}

fn write_guest(
    store: &mut Store<()>,
    instance: &wasmtime::Instance,
    ptr: usize,
    bytes: &[u8],
) -> Result<(), WasmError> {
    instance
        .get_memory(store.as_context_mut(), "memory")
        .ok_or_else(|| WasmError::Export("memory".to_string()))?
        .write(store.as_context_mut(), ptr, bytes)
        .map_err(|e| WasmError::Call(format!("write guest memory: {e}")))
}

/// Read a plugin module (binary `.wasm` or text `.wat` — wasmtime
/// detects), validate its manifest against this host, enforce the
/// permission grant, and register every offered function into the
/// callable table as `"{id}.{name}"`.
///
/// `grants` is the host's capability set — the permissions this host is
/// willing to see requested. The sandbox grants nothing on its own: a
/// module requesting beyond the grant is refused before anything runs.
///
/// # Errors
/// [`WasmError`] — and nothing registers on any of them.
pub fn load_wasm_plugin(path: &Path, grants: &[String]) -> Result<Arc<WasmPlugin>, WasmError> {
    let bytes =
        std::fs::read(path).map_err(|e| WasmError::Load(format!("{}: {e}", path.display())))?;
    let module = Module::new(engine(), &bytes)
        .map_err(|e| WasmError::Load(format!("{}: {e}", path.display())))?;

    let mut store = Store::new(engine(), ());
    // NO imports bound: the module's whole world is its memory and the
    // bytes the host hands it — that IS the sandbox
    let linker: Linker<()> = Linker::new(engine());
    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(|e| WasmError::Load(format!("instantiate: {e}")))?;

    // the manifest, through the guest's own buffers
    let manifest_ptr = guest_alloc_static(&mut store, &instance, OUT_CAP)?;
    let manifest_fn = instance
        .get_typed_func::<(i32, i32), i32>(store.as_context_mut(), "hyg_manifest")
        .map_err(|e| WasmError::Export(format!("hyg_manifest: {e}")))?;
    let written = manifest_fn
        .call(
            store.as_context_mut(),
            (manifest_ptr as i32, OUT_CAP as i32),
        )
        .map_err(|e| WasmError::Call(format!("hyg_manifest trapped: {e}")))?;
    if written < 0 {
        return Err(WasmError::Manifest(
            "the module refused to emit its manifest".into(),
        ));
    }
    let mut buf = vec![0u8; written as usize];
    instance
        .get_memory(store.as_context_mut(), "memory")
        .ok_or_else(|| WasmError::Export("memory".to_string()))?
        .read(store.as_context_mut(), manifest_ptr, &mut buf)
        .map_err(|e| WasmError::Call(format!("read manifest: {e}")))?;
    let manifest_text = String::from_utf8(buf)
        .map_err(|e| WasmError::Manifest(format!("manifest is not UTF-8: {e}")))?;
    let manifest = PluginManifest::from_json(&manifest_text).map_err(WasmError::Manifest)?;
    if manifest.abi != DYNAMIC_ABI {
        return Err(WasmError::Abi {
            expected: DYNAMIC_ABI,
            found: manifest.abi,
        });
    }

    // permission enforcement at the host boundary — before anything runs
    // or registers
    let missing: Vec<String> = manifest
        .permissions
        .iter()
        .filter(|p| !grants.contains(p))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(WasmError::Permission {
            requested: manifest.permissions.clone(),
            granted: grants.to_vec(),
        });
    }

    // the namespace contract, identical to the native tier
    validate_identifier(&manifest.id).map_err(|e| WasmError::Name(format!("plugin id: {e}")))?;
    for spec in &manifest.functions {
        validate_identifier(&spec.name)
            .map_err(|e| WasmError::Name(format!("function {} name: {e}", spec.name)))?;
    }
    let manifest_var = serde_json::from_str::<Var>(&manifest_text)
        .map_err(|e| WasmError::Manifest(format!("manifest is not a hyg value: {e}")))?;

    let plugin = Arc::new(WasmPlugin {
        module,
        instance: Mutex::new(StoreAndInstance { store, instance }),
        manifest,
        manifest_var,
        granted: grants.to_vec(),
    });

    // register every offered function — the same table, the same face
    let entries: Vec<(String, FunctionMeta, HostFn)> = plugin
        .manifest
        .functions
        .iter()
        .map(|fspec| {
            let name = format!("{}.{}", plugin.manifest.id, fspec.name);
            let meta = FunctionMeta {
                summary: fspec.summary.clone(),
                inputs: fspec.inputs.iter().filter_map(pin_ty).collect(),
                output: pin_ty(&fspec.output).unwrap_or(Ty::Null),
                origin: format!("wasm:{}:{}", plugin.manifest.id, plugin.manifest.version),
            };
            let plugin = Arc::clone(&plugin);
            let fname = fspec.name.clone();
            let host: HostFn = Arc::new(Mutex::new(Box::new(move |args: &[Var]| {
                plugin
                    .call(&fname, args)
                    .map_err(|e| ScriptError::Engine(e.to_string()))
            })));
            (name, meta, host)
        })
        .collect();
    replace_plugin_functions(&plugin.manifest.id, entries);
    Ok(plugin)
}

/// Remove every callable a wasm plugin registered (its `"{id}."`
/// namespace) — the native tier's unload, shared.
pub fn unload_wasm_plugin(plugin_id: &str) {
    replace_plugin_functions(plugin_id, Vec::new());
}

/// The manifest's pin vocabulary → `Var`'s type system. A pin type the
/// host does not know (shouldn't happen — the contract enum is shared)
/// maps to Null, the untyped marker.
fn pin_ty(pin: &PinTy) -> Option<Ty> {
    match pin {
        PinTy::Null => Some(Ty::Null),
        PinTy::Bool => Some(Ty::Bool),
        PinTy::I64 => Some(Ty::I64),
        PinTy::F64 => Some(Ty::F64),
        PinTy::Str => Some(Ty::Str),
        PinTy::List => Some(Ty::List),
        PinTy::Object => Some(Ty::Object),
        PinTy::Func => Some(Ty::Func),
    }
}

/// The load path's alloc (before the plugin struct exists).
fn guest_alloc_static(
    store: &mut Store<()>,
    instance: &wasmtime::Instance,
    size: usize,
) -> Result<usize, WasmError> {
    let alloc = instance
        .get_typed_func::<i32, i32>(store.as_context_mut(), "alloc")
        .map_err(|e| WasmError::Export(format!("alloc: {e}")))?;
    let ptr = alloc
        .call(store.as_context_mut(), size as i32)
        .map_err(|e| WasmError::Call(format!("alloc trapped: {e}")))?;
    if ptr <= 0 {
        return Err(WasmError::Call(format!(
            "alloc returned {ptr} for {size} bytes"
        )));
    }
    Ok(ptr as usize)
}
