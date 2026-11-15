//! The graph → Rust compiler (M4): the native path, end to end. The
//! feature-free tests pin the generated crate's shape and the native
//! vocabulary's contracts; the gated tests run the whole pipeline for
//! real — generate a crate, `rustc` builds it, `load_plugin` loads it
//! through the M1 boundary, and the graph route serves live traffic
//! from the function table. A compiled graph is a plugin.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use hyg::dynamic::functions::{register_function, FunctionMeta};
use hyg::dynamic::graph::model::{Graph, GRAPH_FORMAT};
use hyg::dynamic::graph::to_rust::{
    build_generated, generate, GraphCrateSpec, RustBinding, RustVocabulary,
};
use hyg::var::Ty;
use hyg::Var;

// ---- document helpers (the shape an editor saves) ----

fn node(id: &str, kind: &str) -> Var {
    Var::from(hyg::o! {
        "id": Var::from(id),
        "type": Var::from(kind),
    })
}

fn node_params(id: &str, kind: &str, params: Var) -> Var {
    Var::from(hyg::o! {
        "id": Var::from(id),
        "type": Var::from(kind),
        "params": params,
    })
}

fn wire(from: &str, from_pin: &str, to: &str, to_pin: &str) -> Var {
    Var::from(hyg::o! {
        "from": Var::from(from),
        "from_pin": Var::from(from_pin),
        "to": Var::from(to),
        "to_pin": Var::from(to_pin),
    })
}

/// The demo graph: request → data/get(who) → greet function node →
/// respond. The same shape the JS path's live test serves — one graph,
/// two compilers.
fn greeting_doc() -> Var {
    Var::from(hyg::o! {
        "graph": Var::from("native-greeting"),
        "format": Var::from(GRAPH_FORMAT),
        "nodes": Var::from(hyg::a![
            node("req1", "web/request"),
            node_params("get1", "data/get", Var::from(hyg::o! { "key": Var::from("who") })),
            node("greet1", "native_greet"),
            node("resp1", "web/respond"),
        ]),
        "wires": Var::from(hyg::a![
            wire("req1", "exec", "greet1", "exec"),
            wire("greet1", "exec", "resp1", "exec"),
            wire("req1", "body", "get1", "value"),
            wire("get1", "out", "greet1", "in0"),
            wire("greet1", "out", "resp1", "body"),
        ]),
        "entry": Var::from(hyg::a![Var::from("req1")]),
    })
}

fn spec(hyg_dep: &str) -> GraphCrateSpec {
    GraphCrateSpec {
        plugin_id: "rust_route".into(),
        version: "0.1.0".into(),
        hyg_dep: hyg_dep.to_string(),
        plugin_api_dep: format!(
            "hyg-plugin-api = {{ path = {:?} }}",
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("plugin-api")
                .display()
                .to_string()
        ),
        extra_deps: vec![],
    }
}

fn vocab_with_greet() -> RustVocabulary {
    let mut vocab = RustVocabulary::default();
    vocab.bind(
        "native_greet",
        RustBinding {
            path: "hosts::greet_var".into(),
            inputs: vec![Ty::Str],
            output: Ty::Str,
        },
    );
    vocab
}

// ---- generation shape (feature-free) ----

#[test]
fn native_route_generates_a_self_contained_crate() {
    let doc = greeting_doc();
    let graph = Graph::from_var(&doc).expect("structurally valid");

    let vocab = vocab_with_greet();
    let crate_spec = spec("hyg = { path = \"/repo\" }");
    let generated = generate(&graph, &vocab, &crate_spec).expect("the native route generates");

    assert_eq!(generated.name, "rust_route");
    let cargo = &generated.files[0].1;
    assert!(cargo.contains("crate-type = [\"cdylib\"]"), "{cargo}");
    assert!(cargo.contains("hyg = { path = \"/repo\" }"), "{cargo}");
    let lib = &generated.files[1].1;
    // the contract exports + the Var-native route + node markers
    assert!(
        lib.contains("pub extern \"C\" fn hyg_plugin_manifest"),
        "{lib}"
    );
    assert!(lib.contains("pub extern \"C\" fn hyg_plugin_call"), "{lib}");
    assert!(
        lib.contains("fn route(req: &Var) -> Result<Var, String>"),
        "{lib}"
    );
    assert!(lib.contains("// [hyg:node:req1]"), "{lib}");
    assert!(lib.contains("// [hyg:node:greet1]"), "{lib}");
    // data/get inlines as Var::get; the function node calls the bound path
    assert!(lib.contains("req1_body = req.get(\"body\");"), "{lib}");
    assert!(lib.contains("req1_body.get(\"who\")"), "{lib}");
    assert!(
        lib.contains("greet1_out = hosts::greet_var(&[req1_body.get(\"who\")])?;"),
        "{lib}"
    );
    assert!(lib.contains("return Ok(greet1_out);"), "{lib}");
    // the native path marshals ONCE: the call export parses the request,
    // the route works on Var, the result crosses as one JSON text
    assert!(lib.contains("serde_json::from_str(req_json)"), "{lib}");
    assert!(lib.contains("__hyg_json(&value)"), "{lib}");
}

#[test]
fn unbound_function_nodes_refuse_with_coordinates() {
    let doc = greeting_doc();
    let graph = Graph::from_var(&doc).expect("structurally valid");
    let err = generate(&graph, &RustVocabulary::default(), &spec("hyg = \"0.1\""))
        .expect_err("an unbound function node refuses the native path");
    assert_eq!(
        err.node.as_deref(),
        Some("greet1"),
        "the error names the node to click"
    );
    assert!(err.to_string().contains("no native binding"), "{err}");
}

#[test]
fn bindings_must_agree_with_the_function_table() {
    // the callable lives in the table with signature [Str] → Str; a
    // binding claiming a different signature is a compilers-disagree bug
    register_function(
        "native_greet",
        FunctionMeta {
            summary: String::new(),
            inputs: vec![Ty::Str],
            output: Ty::Str,
            origin: "host:native_greet".into(),
        },
        Arc::new(Mutex::new(Box::new(|_args: &[Var]| Ok(Var::Null)))),
    );
    let doc = greeting_doc();
    let graph = Graph::from_var(&doc).expect("structurally valid");

    let mut vocab = RustVocabulary::default();
    vocab.bind(
        "native_greet",
        RustBinding {
            path: "hosts::greet_var".into(),
            inputs: vec![Ty::Str, Ty::Str],
            output: Ty::Str,
        },
    );
    let err = generate(&graph, &vocab, &spec("hyg = \"0.1\""))
        .expect_err("a disagreeing binding refuses");
    assert!(
        err.to_string()
            .contains("disagrees with the function table"),
        "{err}"
    );
    assert!(err.to_string().contains("must never disagree"), "{err}");
}

// ---- the whole native pipeline (dynamic-ffi + web) ----

#[cfg(all(feature = "dynamic-ffi", feature = "web"))]
mod native_pipeline {
    use std::time::Duration;

    use hyg::dynamic::ffi::load_plugin;
    use hyg::dynamic::functions::{call_function, function_names};
    use hyg::web::{Response, Server};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use super::*;

    /// The project-side functions the graph's function nodes call — the
    /// `fn(&[Var]) -> Result<Var, String>` convention, living in a
    /// dependency crate the generated code declares.
    fn write_hosts_crate(workspace: &std::path::Path, hyg_dep: &str) {
        let src = "// the project's native functions the graph links against\n\
             use hyg::Var;\n\n\
             pub fn greet_var(args: &[Var]) -> Result<Var, String> {\n\
             \x20   let who = args.first().and_then(Var::as_str).unwrap_or_default();\n\
             \x20   Ok(Var::from(format!(\"hello, {who} (native)!\")))\n\
             }\n";
        let dir = workspace.join("hosts");
        std::fs::create_dir_all(dir.join("src")).expect("hosts dir");
        std::fs::write(
            dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"hosts\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{hyg_dep}\n"
            ),
        )
        .expect("hosts Cargo.toml");
        std::fs::write(dir.join("src/lib.rs"), src).expect("hosts lib.rs");
    }

    #[test]
    fn generated_crate_builds_loads_and_serves() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let hyg_dep = format!("hyg = {{ path = {:?} }}", repo.display().to_string());
        let workspace = repo.join("target/plugin-tests/graph-workspace");
        let target_dir = repo.join("target/graph-tests");

        // the generated crate + the project crate it links against
        write_hosts_crate(&workspace, &hyg_dep);
        let doc = greeting_doc();
        let graph = Graph::from_var(&doc).expect("structurally valid");
        let crate_spec = GraphCrateSpec {
            extra_deps: vec![("hosts".into(), "{ path = \"../hosts\" }".into())],
            ..spec(&hyg_dep)
        };
        let generated =
            generate(&graph, &vocab_with_greet(), &crate_spec).expect("the native route generates");

        // rustc does the real work; the artifact is a plugin
        let artifact = build_generated(&generated, &workspace, &target_dir)
            .expect("the generated crate compiles");
        assert!(artifact.exists(), "{artifact:?}");

        // it loads through the M1 boundary like any plugin
        let plugin = load_plugin(&artifact).expect("the compiled graph loads as a plugin");
        assert_eq!(plugin.manifest().id, "rust_route");
        assert_eq!(plugin.manifest().version, "0.1.0");
        assert!(
            function_names().iter().any(|n| n == "rust_route.handle"),
            "the route landed in the function table"
        );

        // and serves: the graph's logic, native, through the table
        let req = Var::from(hyg::o! { "body": Var::from(hyg::o! { "who": Var::from("Rose") }) });
        let out = call_function("rust_route.handle", &[req]).expect("the native route serves");
        assert_eq!(out.as_str(), Some("hello, Rose (native)!"));

        // a failing bound path is a loud error through the same voice
        let err = call_function("rust_route.handle", &[]).expect_err("arity is checked");
        assert!(err.to_string().contains("expected 1 args"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_native_route_serves_live_traffic() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let hyg_dep = format!("hyg = {{ path = {:?} }}", repo.display().to_string());
        let workspace = repo.join("target/plugin-tests/graph-workspace");
        let target_dir = repo.join("target/graph-tests");

        write_hosts_crate(&workspace, &hyg_dep);
        let doc = greeting_doc();
        let graph = Graph::from_var(&doc).expect("structurally valid");
        let crate_spec = GraphCrateSpec {
            extra_deps: vec![("hosts".into(), "{ path = \"../hosts\" }".into())],
            ..spec(&hyg_dep)
        };
        let generated =
            generate(&graph, &vocab_with_greet(), &crate_spec).expect("the native route generates");
        let artifact = build_generated(&generated, &workspace, &target_dir).expect("compiles");
        let _plugin = load_plugin(&artifact).expect("loads");

        // a graph compiled to Rust, served on a live server through the
        // function table — the native path end to end
        let mut serv = Server::with_config(hyg::o! { "host": "127.0.0.1", "port": 0 });
        serv.post("/native-hello", move |req| {
            match call_function("rust_route.handle", &[req]) {
                Ok(v) => Response::text(v.as_string().unwrap_or_default()),
                Err(e) => Response::text(format!("{e}")).status(500),
            }
        });
        let s = serv.clone();
        tokio::spawn(async move {
            let _ = s.start().await;
        });
        let mut addr = String::new();
        for _ in 0..100 {
            addr = serv.local_addr();
            if !addr.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!addr.is_empty(), "server bound");

        let raw = "POST /native-hello HTTP/1.1\r\nhost: t\r\ncontent-type: application/json\r\ncontent-length: 14\r\nconnection: close\r\n\r\n{\"who\":\"Rose\"}";
        let mut s = TcpStream::connect(&addr).await.expect("the server accepts");
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
        assert_eq!((status, body), (200, "hello, Rose (native)!"));
    }

    #[test]
    fn host_api_drift_remaps_to_graph_coordinates() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let hyg_dep = format!("hyg = {{ path = {:?} }}", repo.display().to_string());
        let workspace = repo.join("target/plugin-tests/graph-drift");
        let target_dir = repo.join("target/graph-tests");
        write_hosts_crate(&workspace, &hyg_dep);

        let doc = greeting_doc();
        let graph = Graph::from_var(&doc).expect("structurally valid");
        let crate_spec = GraphCrateSpec {
            extra_deps: vec![("hosts".into(), "{ path = \"../hosts\" }".into())],
            ..spec(&hyg_dep)
        };
        // the drift: the bound path does not exist in the hosts crate —
        // exactly the "rare, only host-API drift" failure the design
        // anticipates; rustc's span remaps back to the function node
        let mut vocab = RustVocabulary::default();
        vocab.bind(
            "native_greet",
            RustBinding {
                path: "hosts::renamed_away".into(),
                inputs: vec![Ty::Str],
                output: Ty::Str,
            },
        );
        let generated = generate(&graph, &vocab, &crate_spec).expect("generates");
        let err = build_generated(&generated, &workspace, &target_dir)
            .expect_err("the drift refuses to build");
        assert_eq!(
            err.node.as_deref(),
            Some("greet1"),
            "rustc's span remapped to the node to click: {err:?}"
        );
        assert!(err.to_string().contains("host-API drift"), "{err}");
    }
}
