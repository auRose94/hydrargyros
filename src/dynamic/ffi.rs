//! FFI plugins — the trusted tier of [`hyg::dynamic`](crate::dynamic)
//! ([dynamic-design.md](../../docs/dynamic-design.md)): native shared
//! libraries whose functions land in the same callable table script
//! functions live in, loadable and **hot-swappable while the process
//! stays up**. The contract lives in the `hyg-plugin-api` crate (shared by
//! host and plugin authors); this file is the host half.
//!
//! # The loading mechanics (and why not `RootModule`)
//!
//! Libraries load through `libloading`, and the values crossing the
//! boundary are abi_stable's layout-stable types (`RString`/`RVec`/
//! `RBoxError`). abi_stable's `RootModule` machinery is deliberately NOT
//! used: `load_from` caches the module in process-global statics (one
//! load per process) and leaks the library by design — both incompatible
//! with hot-swap, where a new version loads side-by-side and the old one
//! unloads after its in-flight calls drain. Layout stability is kept
//! where it matters: in the data types, plus the manifest's ABI version
//! check below.
//!
//! # Hot swap (the watch-based-drain pattern)
//!
//! [`load_plugin`] loads, validates, and registers. [`Plugin::reload`]
//! then: loads the new version from its file (mapping a **private
//! side-by-side copy**, so the build system may rewrite the source file
//! while we run — never over a mapped file), swaps the plugin's function
//! entries in one atomic table write, waits a bounded grace period for
//! in-flight calls on the old version to drain, and releases the old
//! library (unloaded exactly when the last in-flight call releases it —
//! callers hold an `Arc` across the call, so a live call can never be
//! unmapped under itself). A failed load swaps nothing: the previous
//! version keeps serving and the error returns to the caller.
//!
//! # unsafe policy
//!
//! This is the only file in the repo allowed `unsafe` outside the wgpu
//! backends. Every block carries the invariant that makes it sound.
//!
//! # Trusted mode
//!
//! Native FFI can crash the process and sees memory directly — it is the
//! *trusted* tier. The manifest's `permissions` field rides along for the
//! host APIs that can enforce it; the one that exists today (function
//! registration) is enforced by construction: a plugin's functions
//! register into its own `"{id}."` namespace and nowhere else. Anything
//! less trusted belongs in the WASM tier (a later milestone).

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use abi_stable::std_types::{RBoxError, RErr, ROk, RResult, RString, RVec};
use libloading::Library;

use crate::dynamic::engine::{validate_identifier, HostFn, ScriptError};
use crate::dynamic::functions::{replace_plugin_functions, FunctionMeta};
use crate::var::{Ty, Var};

// the contract, shared with plugin authors — re-exported so host callers
// speak one vocabulary (`hyg::dynamic::ffi::PluginManifest`, …)
pub use hyg_plugin_api::{FunctionSpec, PinTy, PluginManifest, DYNAMIC_ABI};

/// The mandated manifest export: the `.json.hyg` Var document as JSON text.
type ManifestExport = unsafe extern "C" fn() -> RString;
/// The mandated dispatch export: one JSON text per arg in, one JSON text out.
type CallExport = unsafe extern "C" fn(RString, RVec<RString>) -> RResult<RString, RBoxError>;

/// Failure of a plugin load, reload, or contract check — every variant is
/// an explicit error with the context to act on, never a silent fallback.
#[derive(Debug)]
pub enum PluginError {
    /// The plugin file could not be copied to its side-by-side scratch path.
    Io(String),
    /// The shared library could not load, or a mandated export was missing
    /// (the OS loader's message included).
    Load(String),
    /// The manifest's `abi` is not this host's [`DYNAMIC_ABI`].
    Abi {
        expected: u64,
        found: u64,
    },
    /// The manifest text is not a valid [`PluginManifest`] document.
    Manifest(String),
    /// A reload handed over a file whose manifest id differs from the
    /// plugin's — reloading a different plugin into a live slot would
    /// strand its namespace, so it refuses.
    IdMismatch {
        expected: String,
        found: String,
    },
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PluginError::Io(msg) => write!(f, "plugin file: {msg}"),
            PluginError::Load(msg) => write!(f, "plugin load: {msg}"),
            PluginError::Abi { expected, found } => write!(
                f,
                "plugin ABI mismatch: host expects {expected}, plugin manifest reports {found} \
                 — rebuild the plugin against this hyg-plugin-api"
            ),
            PluginError::Manifest(msg) => write!(f, "{msg}"),
            PluginError::IdMismatch { expected, found } => write!(
                f,
                "reload mismatch: slot holds plugin {expected:?}, file's manifest reports {found:?}"
            ),
        }
    }
}

impl std::error::Error for PluginError {}

/// One materialized library version — a loaded library, its validated
/// manifest, and the count of calls currently inside its code.
struct LoadedLib {
    /// The mapped library. Dropped = unloaded (dlclose); the field comes
    /// BEFORE `scratch_dir` so the unmap precedes the scratch file's
    /// deletion (Windows cannot delete a mapped DLL).
    #[allow(dead_code)] // never read — it exists for its drop glue (dlclose)
    lib: Library,
    /// The dispatch export, resolved once at load.
    call_fn: CallExport,
    /// The validated manifest (owned — the exported text may be an
    /// interned static in the plugin, and the library can unload).
    manifest: PluginManifest,
    /// The manifest as the `.json.hyg` Var it is — kept for introspection
    /// (the inspector, Studio panels).
    manifest_var: Var,
    /// The side-by-side scratch copy this version maps. Every version maps
    /// its own file, so a rebuild that rewrites the source file never
    /// touches a mapped mapping (and on Windows a DLL file can be replaced
    /// while an old version still runs).
    #[allow(dead_code)] // never read — it exists for its drop glue (cleanup)
    scratch_dir: tempfile::TempDir,
    /// Calls currently inside this library's code. The guard against
    /// unloading under a live call is the `Arc<LoadedLib>` each caller
    /// holds; this counter is the *observable* — the drain poll reads it.
    in_flight: AtomicUsize,
}

impl LoadedLib {
    /// Load, validate, and materialize one library version. Nothing here
    /// registers anything — a failure leaves the host untouched.
    fn load(path: &Path) -> Result<Self, PluginError> {
        // side-by-side: map a private copy, never the file the build
        // system wrote (it may be rewritten while we run)
        let scratch_dir =
            tempfile::tempdir().map_err(|e| PluginError::Io(format!("scratch dir: {e}")))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| PluginError::Io(format!("{}: not a file path", path.display())))?
            .to_owned();
        let copy = scratch_dir.path().join(file_name);
        std::fs::copy(path, &copy).map_err(|e| {
            PluginError::Io(format!("copy {} → {}: {e}", path.display(), copy.display()))
        })?;

        let lib = unsafe {
            // SAFETY: dlopen of the scratch copy — the hazard this call
            // owns is the library's own init code, which is the plugin's
            // contract to keep honest (trusted tier).
            Library::new(&copy)
        }
        .map_err(|e| PluginError::Load(format!("{}: {e}", path.display())))?;

        // resolve the mandated exports (deref copies the fn pointers out;
        // `lib` keeps them mapped for this struct's lifetime)
        let (call_fn, manifest_text) = unsafe {
            // SAFETY: symbol lookup into the library mapped above, whose
            // handle `lib` outlives every use of the resolved pointers.
            let call_fn: CallExport = *lib
                .get(b"hyg_plugin_call")
                .map_err(|e| PluginError::Load(format!("{}: missing export: {e}", path.display())))?;
            let manifest_fn: ManifestExport = *lib
                .get(b"hyg_plugin_manifest")
                .map_err(|e| PluginError::Load(format!("{}: missing export: {e}", path.display())))?;
            // SAFETY: fn pointers from THIS library, still mapped (`lib`
            // lives to the end of this struct); signatures match the
            // mandated exports; the contract's manifest export never
            // unwinds (returns an owned RString).
            let text = manifest_fn();
            (call_fn, text)
        };

        let manifest = PluginManifest::from_json(&manifest_text)
            .map_err(|e| PluginError::Manifest(format!("{}: {e}", path.display())))?;
        if manifest.abi != DYNAMIC_ABI {
            return Err(PluginError::Abi {
                expected: DYNAMIC_ABI,
                found: manifest.abi,
            });
        }

        // the manifest is a `.json.hyg` Var — materialize it for
        // introspection (order-preserving parse, the bridge's way)
        let manifest_var = serde_json::from_str::<Var>(&manifest_text).map_err(|e| {
            PluginError::Manifest(format!("{}: manifest is not a hyg value: {e}", path.display()))
        })?;

        // contract validation: the id namespaces the table, function names
        // are callables — both must be plain ASCII identifiers
        validate_identifier(&manifest.id)
            .map_err(|e| PluginError::Manifest(format!("{}: plugin id: {e}", path.display())))?;
        for spec in &manifest.functions {
            validate_identifier(&spec.name).map_err(|e| {
                PluginError::Manifest(format!(
                    "{}: function {spec:?} name: {e}",
                    path.display()
                ))
            })?;
        }

        Ok(LoadedLib {
            lib,
            call_fn,
            manifest,
            manifest_var,
            scratch_dir,
            in_flight: AtomicUsize::new(0),
        })
    }

    /// Call one of this library's functions: `Var` args → JSON texts →
    /// the dispatch export → JSON text → `Var`. The plugin-side rules
    /// (default allocator, no unwind, no TLS) make every step sound.
    fn call(&self, name: &str, args: &[Var]) -> Result<Var, ScriptError> {
        let mut marshaled: RVec<RString> = RVec::with_capacity(args.len());
        for (i, arg) in args.iter().enumerate() {
            marshaled.push(RString::from(marshal_json(arg).map_err(|e| {
                ScriptError::type_err(format!("{name}: arg {i}: {e}"))
            })?));
        }
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        let out = unsafe {
            // SAFETY: call_fn was resolved from THIS library by load();
            // `self.lib` keeps it mapped for `self`'s lifetime, and every
            // caller holds an `Arc<LoadedLib>` clone across the call (the
            // function-table entry hands one out before invoking), so the
            // code cannot be unmapped mid-call. The plugin contract
            // mandates the export never unwinds (panics are caught
            // plugin-side and returned as `RErr`) — a Rust panic across an
            // `extern "C"` boundary aborts, so none can escape here.
            (self.call_fn)(RString::from(name), marshaled)
        };
        // Relaxed: the counter is an observable for the drain poll, not
        // the guard — the Arc the caller holds IS the guard.
        self.in_flight.fetch_sub(1, Ordering::Relaxed);

        match out {
            ROk(text) => crate::dynamic::engine::json_to_var(&text)
                .map_err(|e| ScriptError::Type(format!("{name}: result: {e}"))),
            RErr(e) => Err(ScriptError::Engine(format!("{name}: {e}"))),
        }
    }
}

/// `Var` → JSON text for the plugin wire, with the JSON-exactness floor:
/// non-finite floats have no JSON spelling and would silently degrade to
/// null — an explicit error instead. (The script bridge's 2^53 integer cap
/// is a JS limit, not a JSON one; a plugin parses the text itself.)
fn marshal_json(v: &Var) -> Result<String, ScriptError> {
    check_json_fidelity(v)?;
    serde_json::to_string(v)
        .map_err(|e| ScriptError::type_err(format!("value cannot cross into the plugin: {e}")))
}

/// Walk a `Var` tree enforcing what JSON holds exactly.
fn check_json_fidelity(v: &Var) -> Result<(), ScriptError> {
    match v {
        Var::F64(f) if !f.is_finite() => Err(ScriptError::type_err(format!(
            "float {f} is not finite — JSON cannot hold it"
        ))),
        Var::List(l) => {
            for item in l.iter().iter() {
                check_json_fidelity(item)?;
            }
            Ok(())
        }
        Var::Object(o) => {
            for (_, item) in o.entries().iter() {
                check_json_fidelity(item)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// A plugin slot: one id, the currently-serving version, and the swap
/// point for reloads. Functions register through the table either way —
/// the handle exists to make reload's contract explicit.
pub struct Plugin {
    id: String,
    current: RwLock<Arc<LoadedLib>>,
}

/// What a successful reload did — the always-up story, reported.
#[derive(Debug)]
pub struct ReloadReport {
    /// The version now serving.
    pub version: String,
    /// The version retired.
    pub retired_version: String,
    /// `false` when the drain grace expired with calls still in flight:
    /// the swap still happened (new calls hit the new version), and the
    /// retired library stays mapped until the last in-flight call
    /// releases it — never unloaded under a live call.
    pub drained: bool,
}

impl Plugin {
    /// The plugin's namespace id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The currently-serving version's manifest.
    #[must_use]
    pub fn manifest(&self) -> PluginManifest {
        self.current
            .read()
            .expect("plugin slot")
            .manifest
            .clone()
    }

    /// The current manifest as the `.json.hyg` Var it is.
    #[must_use]
    pub fn manifest_var(&self) -> Var {
        self.current.read().expect("plugin slot").manifest_var.clone()
    }

    /// The currently-serving version's string.
    #[must_use]
    pub fn version(&self) -> String {
        self.current.read().expect("plugin slot").manifest.version.clone()
    }

    /// Calls currently inside the serving version's code (introspection;
    /// the drain test asserts on it).
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.current
            .read()
            .expect("plugin slot")
            .in_flight
            .load(Ordering::Relaxed)
    }

    /// Hot-swap: load `path` as the new version of THIS plugin and swap it
    /// in — a failed load swaps nothing; a different plugin id refuses.
    /// `drain` bounds the wait for in-flight calls on the old version.
    ///
    /// # Errors
    /// [`PluginError`] from the load, the id check, or (never) the swap —
    /// by the time anything can fail, nothing has changed.
    pub fn reload(&self, path: &Path, drain: Duration) -> Result<ReloadReport, PluginError> {
        // load + validate FIRST — a failure swaps nothing
        let new = Arc::new(LoadedLib::load(path)?);
        if new.manifest.id != self.id {
            return Err(PluginError::IdMismatch {
                expected: self.id.clone(),
                found: new.manifest.id.clone(),
            });
        }

        // the guard's scope ends before any error path (the registry's
        // day-one deadlock: never re-read under a held read guard)
        let old = self.current.read().expect("plugin slot").clone();
        let retired_version = old.manifest.version.clone();

        // atomic swap: the new version's functions replace the namespace
        // under ONE table write (all-old → all-new, never a mix)
        replace_plugin_functions(&self.id, build_entries(&new));
        *self.current.write().expect("plugin slot") = Arc::clone(&new);

        // retire: bounded drain, then release — the library unloads when
        // the LAST holder drops (here if drained, after the last in-flight
        // call otherwise)
        let drained = wait_drained(&old.in_flight, drain);
        drop(old);

        Ok(ReloadReport {
            version: new.manifest.version.clone(),
            retired_version,
            drained,
        })
    }
}

/// Load a plugin, validate its manifest against this host, and register
/// every offered function into the callable table as `"{id}.{name}"` —
/// where script functions and graph function nodes live too.
///
/// # Errors
/// [`PluginError`] — and nothing registers on any of them.
pub fn load_plugin(path: &Path) -> Result<Arc<Plugin>, PluginError> {
    let lib = Arc::new(LoadedLib::load(path)?);
    let plugin = Arc::new(Plugin {
        id: lib.manifest.id.clone(),
        current: RwLock::new(Arc::clone(&lib)),
    });
    replace_plugin_functions(&plugin.id, build_entries(&lib));
    Ok(plugin)
}

/// Remove every callable a plugin registered (its `"{id}."` namespace).
/// Explicit, not drop-driven: entries outlive handles on purpose (a
/// callable keeps its own state alive), so cleanup is a decision.
pub fn unload_plugin(plugin_id: &str) {
    replace_plugin_functions(plugin_id, Vec::new());
}

/// One table entry per offered function: the namespace name, the
/// descriptor (pins mapped onto `Var`'s vocabulary), and a host closure
/// holding the loaded library — the same [`HostFn`] shape scripts bind.
fn build_entries(lib: &Arc<LoadedLib>) -> Vec<(String, FunctionMeta, HostFn)> {
    lib.manifest
        .functions
        .iter()
        .map(|spec| {
            let name = format!("{}.{}", lib.manifest.id, spec.name);
            let meta = FunctionMeta {
                summary: spec.summary.clone(),
                inputs: spec.inputs.iter().map(|p| pin_ty(*p)).collect(),
                output: pin_ty(spec.output),
                origin: format!("plugin:{}:{}", lib.manifest.id, lib.manifest.version),
            };
            let lib = Arc::clone(lib);
            let fname = spec.name.clone();
            let host: HostFn = Arc::new(Mutex::new(Box::new(move |args: &[Var]| {
                lib.call(&fname, args)
            })));
            (name, meta, host)
        })
        .collect()
}

/// The pin vocabulary → `Var`'s own type system (the contract mirrors it
/// because a plugin cannot depend on hyg).
fn pin_ty(pin: PinTy) -> Ty {
    match pin {
        PinTy::Null => Ty::Null,
        PinTy::Bool => Ty::Bool,
        PinTy::I64 => Ty::I64,
        PinTy::F64 => Ty::F64,
        PinTy::Str => Ty::Str,
        PinTy::List => Ty::List,
        PinTy::Object => Ty::Object,
        PinTy::Func => Ty::Func,
    }
}

/// Poll the in-flight counter until it drains or the grace period expires.
fn wait_drained(counter: &AtomicUsize, drain: Duration) -> bool {
    let deadline = Instant::now() + drain;
    while counter.load(Ordering::Relaxed) > 0 {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    true
}
