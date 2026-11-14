//! The boundary-proof test plugin — a small, honest example of the whole
//! contract: two mandated exports, three functions (one per interesting
//! shape), a version baked at build time so the hot-swap tests can reload
//! a rebuilt file and watch behavior change while the host keeps running.
//!
//! See hyg-plugin-api's docs for the author rules this file follows (the
//! default allocator, no unwind across the boundary, no thread_local!).

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Duration;

use abi_stable::std_types::{RBoxError, RResult, RString, RVec};
use hyg_plugin_api::{
    arg, refuse, respond, FunctionSpec, PinTy, PluginManifest, DYNAMIC_ABI,
};
use serde_json::json;

/// The baked identity: build.rs sets `HYG_PLUGIN_VERSION`/`HYG_PLUGIN_ID`
/// (defaults "0.1.0"/"greeter"); `HYG_PLUGIN_ABI` only exists when a test
/// deliberately lies about it.
fn version() -> &'static str {
    env!("HYG_PLUGIN_VERSION")
}

fn id() -> &'static str {
    env!("HYG_PLUGIN_ID")
}

fn abi() -> u64 {
    option_env!("HYG_PLUGIN_ABI")
        .and_then(|s| s.parse().ok())
        .unwrap_or(DYNAMIC_ABI)
}

fn manifest() -> PluginManifest {
    PluginManifest {
        id: id().into(),
        version: version().into(),
        abi: abi(),
        // M1's enforceable seam is registration-into-own-namespace, enforced
        // by the host by construction — the field demonstrates the shape
        permissions: vec!["register:own".into()],
        functions: vec![
            FunctionSpec {
                name: "greet".into(),
                summary: "a versioned greeting (the swap probe: the text changes per build)"
                    .into(),
                inputs: vec![PinTy::Str],
                output: PinTy::Str,
            },
            FunctionSpec {
                name: "add".into(),
                summary: "adds two integers (typed pins)".into(),
                inputs: vec![PinTy::I64, PinTy::I64],
                output: PinTy::I64,
            },
            FunctionSpec {
                name: "slow_echo".into(),
                summary: "sleeps `ms`, then echoes `text` (the drain probe)".into(),
                inputs: vec![PinTy::I64, PinTy::Str],
                output: PinTy::Str,
            },
        ],
    }
}

/// Mandated export #1: the manifest as JSON text (the `.json.hyg` Var).
#[no_mangle]
pub extern "C" fn hyg_plugin_manifest() -> RString {
    RString::from(manifest().to_json())
}

// ---- the offered functions: JSON args in, a serde value out ----

fn greet(args: &RVec<RString>) -> Result<serde_json::Value, String> {
    let who: String = arg(args, 0)?;
    Ok(json!(format!("hello, {who} (greeter {})", version())))
}

fn add(args: &RVec<RString>) -> Result<serde_json::Value, String> {
    let a: i64 = arg(args, 0)?;
    let b: i64 = arg(args, 1)?;
    Ok(json!(a + b))
}

fn slow_echo(args: &RVec<RString>) -> Result<serde_json::Value, String> {
    let ms: i64 = arg(args, 0)?;
    let text: String = arg(args, 1)?;
    std::thread::sleep(Duration::from_millis(ms.unsigned_abs()));
    Ok(json!(format!("{text} (greeter {})", version())))
}

/// Mandated export #2: dispatch by name. Never unwinds — a panic is caught
/// at the boundary and reported as an error (the author rule that keeps a
/// plugin panic from aborting the host process).
#[no_mangle]
pub extern "C" fn hyg_plugin_call(
    name: RString,
    args: RVec<RString>,
) -> RResult<RString, RBoxError> {
    let dispatched = catch_unwind(AssertUnwindSafe(|| match name.as_str() {
        "greet" => greet(&args),
        "add" => add(&args),
        "slow_echo" => slow_echo(&args),
        other => Err(format!("unknown function {other:?}")),
    }));
    match dispatched {
        Ok(Ok(value)) => respond(&value),
        Ok(Err(msg)) => refuse(msg),
        Err(_) => refuse(format!("{} panicked (caught at the boundary)", name.as_str())),
    }
}
