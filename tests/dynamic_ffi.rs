//! FFI-plugin boundary tests (the `dynamic-ffi` feature): real shared
//! libraries built on demand from the out-of-tree test plugin
//! (`tests/ffi-plugin/`), loaded for real, hot-swapped for real — v1
//! rebuilt into v2 while calls are in flight, and under live HTTP traffic.
//!
//! Every test uses its OWN plugin id: the function table is process-global
//! and tests run in parallel, so namespaces stay isolated. Builds are
//! serialized (nested `cargo build` invocations each take their target
//! dir's lock) and bake the version/id via env, so one plugin source
//! produces the distinct versions the swap tests need.

#![cfg(feature = "dynamic-ffi")]

use std::env::consts::{DLL_PREFIX, DLL_SUFFIX};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use hyg::dynamic::ffi::{load_plugin, unload_plugin, DYNAMIC_ABI, PinTy, PluginError};
use hyg::dynamic::functions::{call_function, function_meta, function_names};
use hyg::Var;

// ---- the plugin builder ----

/// One nested `cargo build` of the out-of-tree test plugin, serialized
/// across parallel tests, with the manifest identity baked via env. Returns
/// the built library's path.
fn build_plugin(envs: &[(&str, &str)]) -> PathBuf {
    static BUILD_LOCK: Mutex<()> = Mutex::new(());
    let _guard = BUILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest = manifest_dir.join("tests").join("ffi-plugin");
    let target = manifest_dir.join("target").join("plugin-tests");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.arg("build")
        .arg("-q")
        .arg("--manifest-path")
        .arg(manifest.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("cargo spawns");
    assert!(
        out.status.success(),
        "test-plugin build failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    target
        .join("debug")
        .join(format!("{DLL_PREFIX}hyg_test_greeter{DLL_SUFFIX}"))
}

/// A garbage file where a library should be — the loader must refuse it
/// with the OS loader's message, not a panic.
fn corrupt_file(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hyg-test-{name}-{DLL_SUFFIX}"));
    std::fs::write(&path, b"definitely not a shared library").expect("corrupt file writes");
    path
}

/// Wait (bounded) until the current version has `at_least` calls inside its
/// code — makes "the in-flight call had entered" deterministic before a
/// swap, instead of sleep-and-hope.
fn wait_entered(plugin: &hyg::dynamic::ffi::Plugin, at_least: usize, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while plugin.in_flight() < at_least {
        assert!(Instant::now() < deadline, "{what} never entered the plugin");
        thread::sleep(Duration::from_millis(2));
    }
}

// ---- load, manifest, registration, calls ----

#[test]
fn manifest_loads_functions_register_and_calls_flow() {
    let so = build_plugin(&[("HYG_PLUGIN_ID", "greeter_basic")]);
    let plugin = load_plugin(&so).expect("the greeter loads");

    // the manifest, as loaded — the contract's fields, verified
    let manifest = plugin.manifest();
    assert_eq!(manifest.id, "greeter_basic");
    assert_eq!(manifest.version, "0.1.0");
    assert_eq!(manifest.abi, DYNAMIC_ABI);
    assert_eq!(manifest.functions.len(), 3);
    assert_eq!(manifest.functions[0].name, "greet");
    assert_eq!(manifest.functions[0].inputs, vec![PinTy::Str]);
    assert_eq!(manifest.functions[0].output, PinTy::Str);

    // the manifest is a `.json.hyg` Var too — kept for introspection
    assert_eq!(plugin.manifest_var().get("id").as_str(), Some("greeter_basic"));
    assert_eq!(plugin.manifest_var().get("version").as_str(), Some("0.1.0"));

    // registration: "{id}.{fn}" entries in the shared callable table,
    // pins mapped onto Var's type vocabulary
    for name in [
        "greeter_basic.greet",
        "greeter_basic.add",
        "greeter_basic.slow_echo",
    ] {
        assert!(
            function_names().iter().any(|n| n == name),
            "{name} is registered"
        );
    }
    let meta = function_meta("greeter_basic.add").expect("add has a descriptor");
    assert_eq!(meta.inputs.len(), 2, "two input pins");
    assert_eq!(meta.output.to_string(), "int", "pins map onto Ty's names");
    assert_eq!(meta.origin, "plugin:greeter_basic:0.1.0");

    // calls flow through the table
    let out = call_function("greeter_basic.greet", &[Var::from("Rose")]).expect("greet");
    assert_eq!(out.as_str(), Some("hello, Rose (greeter 0.1.0)"));
    let sum = call_function("greeter_basic.add", &[Var::from(2i64), Var::from(3i64)])
        .expect("add");
    assert_eq!(sum, Var::from(5i64));

    // explicit failures: an unknown name lists what IS registered, and a
    // declared signature checks arity before the call
    let err = call_function("greeter_basic.nothing", &[])
        .expect_err("unknown function is an error");
    assert!(err.to_string().contains("unknown function"), "{err}");
    assert!(err.to_string().contains("greeter_basic.greet"), "{err}");
    let err = call_function("greeter_basic.add", &[Var::from(1i64)])
        .expect_err("arity is checked against the declared pins");
    assert!(err.to_string().contains("expected 2 args"), "{err}");

    // explicit unregister: the namespace goes, calls after it are errors
    unload_plugin("greeter_basic");
    assert!(
        !function_names().iter().any(|n| n == "greeter_basic.greet"),
        "unregister removes the namespace"
    );
    assert!(call_function("greeter_basic.greet", &[Var::Null]).is_err());
}

#[test]
fn abi_mismatch_and_corrupt_files_refuse_and_swap_nothing() {
    let so = build_plugin(&[("HYG_PLUGIN_ID", "greeter_abi")]);
    let _plugin = load_plugin(&so).expect("the honest plugin loads");
    let before = call_function("greeter_abi.greet", &[Var::from("x")])
        .expect("v1 serves");

    // a plugin lying about its ABI is refused — and registers NOTHING over
    // the serving version (a failed load swaps nothing)
    let liar = build_plugin(&[("HYG_PLUGIN_ID", "greeter_abi"), ("HYG_PLUGIN_ABI", "999")]);
    let err = load_plugin(&liar).err().expect("the liar refuses");
    match &err {
        PluginError::Abi { expected, found } => {
            assert_eq!(*expected, DYNAMIC_ABI);
            assert_eq!(*found, 999);
        }
        other => panic!("expected PluginError::Abi, got {other:?}"),
    }
    assert_eq!(
        call_function("greeter_abi.greet", &[Var::from("x")]).expect("still serves"),
        before,
        "the refused load left the serving version untouched"
    );

    // a corrupt file refuses with the loader's message (and no panic)
    let corrupt = corrupt_file("abi");
    let err = load_plugin(&corrupt).err().expect("corrupt refuses");
    assert!(matches!(err, PluginError::Load(_)), "{err:?}");
    let _ = std::fs::remove_file(&corrupt);

    unload_plugin("greeter_abi");
}

// ---- hot swap: behavior changes under the same process ----

#[test]
fn reload_swaps_behavior_and_drains_in_flight() {
    let id = "greeter_swap";
    let v1 = build_plugin(&[("HYG_PLUGIN_ID", id)]);
    let plugin = load_plugin(&v1).expect("v1 loads");
    assert_eq!(plugin.version(), "0.1.0");
    assert_eq!(
        call_function(&format!("{id}.greet"), &[Var::from("v?")])
            .expect("v1 greet")
            .as_str(),
        Some("hello, v? (greeter 0.1.0)")
    );

    // an in-flight call on v1 (slow_echo sleeps 300ms) — the reload must
    // drain it: the call runs on v1 to completion, the swap waits for it
    let t = thread::spawn(|| {
        call_function(
            "greeter_swap.slow_echo",
            &[Var::from(300i64), Var::from("still v1")],
        )
        .expect("the in-flight call survives the swap")
    });
    wait_entered(&plugin, 1, "the slow call");

    // rebuild IN PLACE: same output path, new content — the side-by-side
    // scratch copy is what makes mapping the new version safe
    let v2 = build_plugin(&[("HYG_PLUGIN_ID", id), ("HYG_PLUGIN_VERSION", "0.2.0")]);
    let report = plugin.reload(&v2, Duration::from_secs(5)).expect("reload");
    assert_eq!(report.version, "0.2.0");
    assert_eq!(report.retired_version, "0.1.0");
    assert!(report.drained, "the in-flight v1 call drained within the grace");

    let out = t.join().expect("the v1 call finished");
    assert_eq!(
        out.as_str(),
        Some("still v1 (greeter 0.1.0)"),
        "the in-flight call ran on v1 to completion"
    );
    assert_eq!(plugin.in_flight(), 0, "drained");

    // new calls hit v2 — same process, same table, new behavior
    assert_eq!(
        call_function(&format!("{id}.greet"), &[Var::from("v?")])
            .expect("v2 greet")
            .as_str(),
        Some("hello, v? (greeter 0.2.0)")
    );

    // a failed reload swaps nothing — the serving version keeps serving
    let corrupt = corrupt_file("reload");
    let err = plugin
        .reload(&corrupt, Duration::from_secs(1))
        .expect_err("a corrupt reload refuses");
    assert!(matches!(err, PluginError::Load(_)), "{err:?}");
    assert_eq!(plugin.version(), "0.2.0", "the failed swap changed nothing");
    assert_eq!(
        call_function(&format!("{id}.greet"), &[Var::from("v?")])
            .expect("still v2")
            .as_str(),
        Some("hello, v? (greeter 0.2.0)")
    );
    let _ = std::fs::remove_file(&corrupt);

    // a different plugin's id refuses to land in this slot
    let other = build_plugin(&[("HYG_PLUGIN_ID", "greeter_other")]);
    let err = plugin
        .reload(&other, Duration::from_secs(1))
        .expect_err("cross-plugin reload refuses");
    assert!(matches!(err, PluginError::IdMismatch { .. }), "{err:?}");
    assert_eq!(plugin.version(), "0.2.0");

    unload_plugin(id);
}

#[test]
fn drain_timeout_never_unloads_under_a_live_call() {
    let id = "greeter_drain";
    let v1 = build_plugin(&[("HYG_PLUGIN_ID", id)]);
    let plugin = load_plugin(&v1).expect("v1 loads");

    // a call that outlives the grace period: the swap STILL happens (new
    // calls hit v2 immediately), the old library stays mapped until the
    // old call finishes — never unloaded under a live call
    let t = thread::spawn(|| {
        call_function(
            "greeter_drain.slow_echo",
            &[Var::from(900i64), Var::from("long call")],
        )
        .expect("the outlived call completes correctly")
    });
    wait_entered(&plugin, 1, "the long call");

    let v2 = build_plugin(&[("HYG_PLUGIN_ID", id), ("HYG_PLUGIN_VERSION", "0.2.0")]);
    let report = plugin
        .reload(&v2, Duration::from_millis(60))
        .expect("the swap itself always happens on a good load");
    assert!(!report.drained, "the 900ms call outlives the 60ms grace");
    assert_eq!(report.version, "0.2.0");
    assert_eq!(report.retired_version, "0.1.0");

    // new calls already route to v2 while the old call runs
    assert_eq!(
        call_function(&format!("{id}.greet"), &[Var::from("v?")])
            .expect("v2 greet")
            .as_str(),
        Some("hello, v? (greeter 0.2.0)")
    );

    // the retired version's call completes correctly — its Arc held the
    // library mapped across the whole call
    let out = t.join().expect("the long call finished");
    assert_eq!(
        out.as_str(),
        Some("long call (greeter 0.1.0)"),
        "the retired version's in-flight call ran to completion"
    );
    assert_eq!(plugin.in_flight(), 0);
    unload_plugin(id);
}

// ---- hot swap under live HTTP traffic (the always-up story, end to end) ----

#[cfg(feature = "web")]
mod live_server {
    use std::time::Duration;

    use hyg::web::{Response, Server};
    use hyg::{o, Var};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use super::*;

    // the tiny HTTP client from the web-server tests: one request per
    // connection, read-to-end is the end of the body
    async fn http(addr: &str, path: &str) -> (u16, String) {
        let mut s = TcpStream::connect(addr)
            .await
            .expect("the server accepts");
        let raw = format!("GET {path} HTTP/1.1\r\nhost: t\r\nconnection: close\r\n\r\n");
        s.write_all(raw.as_bytes()).await.expect("request writes");
        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut buf))
            .await
            .expect("the response arrives in time")
            .expect("the response reads");
        let text = String::from_utf8_lossy(&buf).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (status, body.to_string())
    }

    /// Spawn `start()` on a clone (clones share the stop signal), wait for
    /// the bind, return the resolved addr.
    async fn boot(serv: &Server) -> String {
        let s = serv.clone();
        tokio::spawn(async move { let _ = s.start().await; });
        for _ in 0..100 {
            let addr = serv.local_addr();
            if !addr.is_empty() {
                return addr;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("server did not bind in time");
    }

    fn plugin_route(id: &'static str, function: &'static str) -> impl Fn(Var) -> Response {
        move |req| {
            let who = req.get("params").get("who").as_string().unwrap_or_default();
            match call_function(&format!("{id}.{function}"), &[Var::from(who)]) {
                Ok(v) => Response::text(v.as_string().unwrap_or_default()),
                Err(e) => Response::text(format!("{e}")).status(500),
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hot_swap_happens_while_the_server_serves() {
        let id = "greeter_live";
        let v1 = build_plugin(&[("HYG_PLUGIN_ID", id)]);
        let plugin = load_plugin(&v1).expect("v1 loads");

        let mut serv = Server::with_config(o! { "host": "127.0.0.1", "port": 0 });
        serv.get("/greet/:who", plugin_route(id, "greet")).get(
            "/slow/:ms",
            move |req| {
                // slow_echo's pins are (ms, text) — the path carries the ms
                let ms: i64 = req
                    .get("params")
                    .get("ms")
                    .as_string()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(400);
                match call_function(
                    &format!("{id}.slow_echo"),
                    &[Var::from(ms), Var::from("live swap")],
                ) {
                    Ok(v) => Response::text(v.as_string().unwrap_or_default()),
                    Err(e) => Response::text(format!("{e}")).status(500),
                }
            },
        );
        let addr = boot(&serv).await;

        // v1 serves real traffic
        let (status, body) = http(&addr, "/greet/Rose").await;
        assert_eq!((status, body.as_str()), (200, "hello, Rose (greeter 0.1.0)"));

        // the slow route serves too (a full 400ms round trip, synchronously
        // probed before the in-flight dance below)
        let (status, body) = http(&addr, "/slow/50").await;
        assert_eq!((status, body.as_str()), (200, "live swap (greeter 0.1.0)"));

        // live traffic mid-swap: a slow request enters v1, the reload
        // swaps and drains it, the request completes on v1, the next
        // request hits v2 — the pump never stops
        let slow_addr = addr.clone();
        let slow = tokio::spawn(async move { http(&slow_addr, "/slow/400").await });
        // bounded but generous: under full-suite parallel load the request
        // can take a while to get dispatched — never sleep-and-hope on a
        // 1s window
        let entered = tokio::time::timeout(Duration::from_secs(30), async {
            while plugin.in_flight() < 1 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await;
        if entered.is_err() {
            // diagnose the world instead of asserting blind: a panicked
            // task (transport) vs a stalled dispatch (load) read
            // differently
            let task_state = if slow.is_finished() { "finished" } else { "still running" };
            panic!(
                "the slow request never entered v1 in 30s (task {task_state}, in_flight {}, addr {addr})",
                plugin.in_flight()
            );
        }

        let v2 = build_plugin(&[("HYG_PLUGIN_ID", id), ("HYG_PLUGIN_VERSION", "0.2.0")]);
        let report = plugin.reload(&v2, Duration::from_secs(5)).expect("reload");
        assert!(report.drained, "the live request drained within the grace");
        assert_eq!(report.retired_version, "0.1.0");
        assert_eq!(report.version, "0.2.0");

        let (status, body) = slow.await.expect("the slow task joins");
        assert_eq!(
            (status, body.as_str()),
            (200, "live swap (greeter 0.1.0)"),
            "the in-flight request served by v1 completed after the swap"
        );
        let (status, body) = http(&addr, "/greet/Rose").await;
        assert_eq!(
            (status, body.as_str()),
            (200, "hello, Rose (greeter 0.2.0)"),
            "the same URL now serves v2"
        );

        unload_plugin(id);
    }
}
