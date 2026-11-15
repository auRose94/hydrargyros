//! The WASM plugin tier (M6): a sandboxed plugin compiles and runs, and
//! permissions are enforced at the host boundary before anything
//! registers. The test plugin is authored as WAT — wasmtime parses text
//! modules, so the boundary proof needs no wasm32 toolchain: the module
//! is a bump-allocator + manifest export + dispatcher whose `echo`
//! copies the args text verbatim (the full Var → JSON → guest-memory →
//! JSON → Var round trip) and whose `greet` returns a fixed response
//! (dispatch through the shared function table).
//!
//! The trust story these tests pin: the module's whole world is its own
//! memory and the bytes the host hands it (no imports are bound — no
//! WASI, no fs, no net), the manifest's `permissions` must fit the
//! host's grant before anything registers, and an ABI-liar module is
//! refused like its native sibling.

use std::path::PathBuf;

use hyg::dynamic::functions::{call_function, function_meta, function_names};
use hyg::dynamic::wasm::{
    load_wasm_plugin, unload_wasm_plugin, FunctionSpec, PinTy as ContractPinTy, PluginManifest,
    WasmError, DYNAMIC_ABI,
};

use hyg::Var;

/// The demo plugin's manifest — the SAME contract document the native
/// tier speaks, generated (and its length measured) in Rust so the WAT
/// carries no hand-counted constants.
fn demo_manifest(abi: u64) -> PluginManifest {
    PluginManifest {
        id: "wasm_demo".into(),
        version: "0.1.0".into(),
        abi,
        permissions: vec!["wasm:demo".into()],
        functions: vec![
            FunctionSpec {
                name: "echo".into(),
                summary: "echoes the raw args JSON (the full wire round trip)".into(),
                inputs: vec![], // undeclared signature — any args accepted
                output: ContractPinTy::Null, // the untyped marker
            },
            FunctionSpec {
                name: "greet".into(),
                summary: "a fixed wasm greeting (dispatch proof)".into(),
                inputs: vec![ContractPinTy::Str],
                output: ContractPinTy::Str,
            },
        ],
    }
}

/// WAT string escaping (a WAT string's escape rules cover JSON's).
fn wat_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The demo module as WAT: a bump allocator over its own heap, the
/// manifest export, and a dispatcher whose two functions need no JSON
/// parsing inside the sandbox — `echo` copies the args text verbatim,
/// `greet` copies a fixed response. Dispatch-by-name-length is a demo
/// shortcut: the HOST resolves names against the manifest, so unknown
/// functions never reach the module.
fn wat_plugin(manifest_json: &str) -> String {
    let manifest_len = manifest_json.len();
    let greet_json = "\"hello from wasm\"";
    let greet_len = greet_json.len();
    let greet_offset = 4096;
    let manifest_offset = 1024;
    format!(
        r#"(module
  ;; two data segments at fixed offsets; the guest heap grows past them
  (memory (export "memory") 8)
  (global $heap (mut i32) (i32.const 65536))
  (data (i32.const {manifest_offset}) "{manifest_escaped}")
  (data (i32.const {greet_offset}) "{greet_escaped}")

  ;; the guest's own bump allocator — the host writes ONLY into what
  ;; alloc returns
  (func (export "alloc") (param $size i32) (result i32)
    (local $ptr i32)
    (global.get $heap)
    (local.set $ptr)
    (global.get $heap)
    (local.get $size)
    (i32.add)
    (global.set $heap)
    (local.get $ptr))

  ;; the manifest: the .json.hyg Var document, copied into the host's
  ;; buffer (length known at build time — the test generated it)
  (func (export "hyg_manifest") (param $out i32) (param $cap i32) (result i32)
    (if (i32.lt_u (local.get $cap) (i32.const {manifest_len}))
      (then (return (i32.const -1))))
    (memory.copy (local.get $out) (i32.const {manifest_offset}) (i32.const {manifest_len}))
    (i32.const {manifest_len}))

  ;; the dispatcher — same shape as the native contract's hyg_plugin_call:
  ;; (name, name_len, args_json, args_len, out, cap) -> written_len (-1: refuse)
  (func (export "hyg_call") (param $name_ptr i32) (param $name_len i32)
        (param $args_ptr i32) (param $args_len i32)
        (param $out i32) (param $cap i32) (result i32)
    (if (i32.eq (local.get $name_len) (i32.const 4))
      (then
        (if (i32.lt_u (local.get $cap) (local.get $args_len))
          (then (return (i32.const -1))))
        (memory.copy (local.get $out) (local.get $args_ptr) (local.get $args_len))
        (return (local.get $args_len))))
    (if (i32.eq (local.get $name_len) (i32.const 5))
      (then
        (if (i32.lt_u (local.get $cap) (i32.const {greet_len}))
          (then (return (i32.const -1))))
        (memory.copy (local.get $out) (i32.const {greet_offset}) (i32.const {greet_len}))
        (return (i32.const {greet_len}))))
    (return (i32.const -1)))
)
"#,
        manifest_escaped = wat_escape(manifest_json),
        greet_escaped = wat_escape(greet_json),
        manifest_len = manifest_len,
        greet_len = greet_len,
        manifest_offset = manifest_offset,
        greet_offset = greet_offset,
    )
}

/// Write a demo module variant and return its path.
fn write_plugin(name: &str, abi: u64) -> PathBuf {
    let dir = std::env::temp_dir().join("hyg-wasm-plugins");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let path = dir.join(format!("{name}.wat"));
    let mut manifest = demo_manifest(abi);
    if name != "demo" {
        // a distinct namespace per variant — the table is process-global
        // and tests run in parallel
        manifest.id = name.replace('-', "_");
    }
    std::fs::write(&path, wat_plugin(&manifest.to_json())).expect("the module writes");
    path
}

fn demo_grants() -> Vec<String> {
    vec!["wasm:demo".into()]
}

// ---- load, manifest, calls: the boundary round trip ----

#[test]
fn wasm_plugin_loads_registers_and_calls() {
    let path = write_plugin("demo", DYNAMIC_ABI);
    let plugin = load_wasm_plugin(&path, &demo_grants()).expect("the sandboxed plugin loads");

    // the manifest, validated against the same contract as native plugins
    let manifest = plugin.manifest();
    assert_eq!(manifest.id, "wasm_demo");
    assert_eq!(manifest.version, "0.1.0");
    assert_eq!(manifest.abi, DYNAMIC_ABI);
    assert_eq!(manifest.functions.len(), 2);
    assert!(manifest.permissions.contains(&"wasm:demo".to_string()));
    // the manifest is a `.json.hyg` Var too
    assert_eq!(plugin.manifest_var().get("id").as_str(), Some("wasm_demo"));
    assert_eq!(plugin.granted(), &["wasm:demo".to_string()]);

    // registered into the SAME function table — "{id}.{name}"
    for name in ["wasm_demo.echo", "wasm_demo.greet"] {
        assert!(
            function_names().iter().any(|n| n == name),
            "{name} registered"
        );
    }
    let meta = function_meta("wasm_demo.greet").expect("greet has a descriptor");
    assert_eq!(meta.origin, "wasm:wasm_demo:0.1.0");

    // greet: the host marshals the arg (the module ignores it — dispatch
    // through the shared table is what's proven)
    let out = call_function("wasm_demo.greet", &[Var::from("Rose")]).expect("greet serves");
    assert_eq!(out.as_str(), Some("hello from wasm"));

    // echo: the FULL wire round trip — Var args → one JSON array text →
    // guest memory → verbatim copy → JSON text → Vars. Insertion-ordered
    // objects and nested shapes survive.
    let args = vec![
        Var::from(5i64),
        Var::from("x"),
        Var::from(hyg::o! { "k": Var::from("v"), "n": Var::from(1i64) }),
    ];
    let echoed = call_function("wasm_demo.echo", &args).expect("echo serves");
    assert_eq!(
        echoed.as_list().expect("a list").len(),
        3,
        "the args round trip"
    );
    assert_eq!(
        echoed.as_list().expect("a list").iter()[2]
            .get("k")
            .as_str(),
        Some("v")
    );

    // explicit failures: an unknown name never reaches the module (the
    // HOST resolves against the manifest), and arity follows the pins
    let err = call_function("wasm_demo.nothing", &[]).expect_err("unknown names are table errors");
    assert!(err.to_string().contains("unknown function"), "{err}");
    let err = call_function("wasm_demo.greet", &[])
        .expect_err("arity is checked against the declared pins");
    assert!(err.to_string().contains("expected 1 args"), "{err}");

    unload_wasm_plugin("wasm_demo");
    assert!(
        !function_names().iter().any(|n| n == "wasm_demo.greet"),
        "unregister removes the namespace"
    );
}

// ---- the trust boundary: permissions and the ABI gate ----

#[test]
fn permissions_exceeding_the_grant_refuse_before_anything_registers() {
    let path = write_plugin("wasm-perm", DYNAMIC_ABI);
    // the host grants NOTHING — the module wants wasm:demo
    let Err(err) = load_wasm_plugin(&path, &[]) else {
        panic!("a manifest exceeding the grant refuses at the boundary");
    };
    match &err {
        WasmError::Permission { requested, granted } => {
            assert_eq!(requested, &["wasm:demo".to_string()]);
            assert!(granted.is_empty());
        }
        other => panic!("expected WasmError::Permission, got {other:?}"),
    }
    assert!(
        !function_names().iter().any(|n| n == "wasm_perm.greet"),
        "a refused plugin registers NOTHING (its own namespace)"
    );
}

#[test]
fn an_abi_liar_refuses_like_its_native_sibling() {
    let path = write_plugin("liar", 999);
    let Err(err) = load_wasm_plugin(&path, &demo_grants()) else {
        panic!("an ABI mismatch refuses before anything registers");
    };
    match &err {
        WasmError::Abi { expected, found } => {
            assert_eq!(*expected, DYNAMIC_ABI);
            assert_eq!(*found, 999);
        }
        other => panic!("expected WasmError::Abi, got {other:?}"),
    }
}

#[test]
fn a_module_missing_the_mandated_exports_refuses() {
    let dir = std::env::temp_dir().join("hyg-wasm-plugins");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let path = dir.join("gutless.wat");
    std::fs::write(&path, "(module (memory (export \"memory\") 1))").expect("writes");
    let Err(err) = load_wasm_plugin(&path, &demo_grants()) else {
        panic!("a module without the contract refuses");
    };
    assert!(matches!(err, WasmError::Export(_)), "{err:?}");
}

#[test]
fn a_broken_allocator_refuses_with_a_loud_call_error() {
    let dir = std::env::temp_dir().join("hyg-wasm-plugins");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let path = dir.join("zero-alloc.wat");
    std::fs::write(
        &path,
        "(module (func (export \"alloc\") (param i32) (result i32) (i32.const 0)))",
    )
    .expect("writes");
    // alloc returning 0 is the module breaking its own contract — the
    // host refuses loudly instead of writing into memory it does not own
    let Err(err) = load_wasm_plugin(&path, &demo_grants()) else {
        panic!("a broken allocator refuses");
    };
    assert!(matches!(err, WasmError::Call(_)), "{err:?}");
}
