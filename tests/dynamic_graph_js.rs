//! The graph → JS compiler (M3): conversion IS validation. The
//! feature-free tests pin codegen shape and the conversion failures'
//! `{node, pin, flow}` coordinates against a real function table; the
//! gated module runs graph-defined routes through a live web server —
//! generated JS in a v12 session, function nodes dispatching into the
//! function table.

use std::sync::{Arc, Mutex};

#[cfg(all(feature = "script", feature = "web"))]
use hyg::dynamic::engine::{HostFn, ScriptError};
use hyg::dynamic::functions::{register_function, FunctionMeta};
use hyg::dynamic::graph::model::{Graph, GraphError, GRAPH_FORMAT};
use hyg::dynamic::graph::to_js::{compile, CompiledRoute, NODE_CALL_HOST, ROUTE_FN};
use hyg::var::Ty;
use hyg::Var;

// ---- document helpers (raw Var docs — the shape an editor saves) ----

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

fn doc_from(nodes: Vec<Var>, wires: Vec<Var>, entry: Vec<&str>) -> Var {
    let nodes_v: hyg::List = nodes.into_iter().collect();
    let wires_v: hyg::List = wires.into_iter().collect();
    let entry_v: hyg::List = entry.into_iter().map(Var::from).collect();
    Var::from(hyg::o! {
        "graph": Var::from("route-doc"),
        "format": Var::from(GRAPH_FORMAT),
        "nodes": Var::from(nodes_v),
        "wires": Var::from(wires_v),
        "entry": Var::from(entry_v),
    })
}

fn compile_doc(doc: Var) -> Result<CompiledRoute, GraphError> {
    let graph = Graph::from_var(&doc).expect("the doc is structurally valid");
    compile(&graph)
}

/// Register a host callable (the table is process-global and tests run
/// in parallel — same-name registration replaces, unique names isolate).
fn callable(
    name: &'static str,
    inputs: Vec<Ty>,
    output: Ty,
    f: impl Fn(&[Var]) -> Var + Send + 'static,
) {
    register_function(
        name,
        FunctionMeta {
            summary: String::new(),
            inputs,
            output,
            origin: format!("host:{name}"),
        },
        Arc::new(Mutex::new(Box::new(move |args: &[Var]| Ok(f(args))))),
    );
}

/// Register a callable that FAILS — the table's Result voice, the shape
/// a plugin error or a script throw takes.
#[cfg(all(feature = "script", feature = "web"))]
fn failing_callable(name: &'static str, inputs: Vec<Ty>, output: Ty) {
    let host: HostFn = Arc::new(Mutex::new(Box::new(
        move |_args: &[Var]| -> Result<Var, ScriptError> {
            Err(ScriptError::Engine(format!(
                "{name} exploded (as designed)"
            )))
        },
    )));
    register_function(
        name,
        FunctionMeta {
            summary: String::new(),
            inputs,
            output,
            origin: format!("host:{name}"),
        },
        host,
    );
}

// ---- codegen shape ----

#[test]
fn a_route_graph_compiles_to_a_handle_function() {
    callable("js_pass", vec![], Ty::Null, |_| Var::Null);
    let doc = doc_from(
        vec![node("req1", "web/request"), node("resp1", "web/respond")],
        vec![
            wire("req1", "exec", "resp1", "exec"),
            wire("req1", "body", "resp1", "body"),
        ],
        vec!["req1"],
    );
    let compiled = compile_doc(doc).expect("a pass-through route converts");
    assert_eq!(compiled.graph_name(), "route-doc");

    // the shape: one handle function taking the staged request; the
    // request node's pins are function-scope lets; respond is a return
    let js = &compiled.js;
    assert!(js.contains(&format!("function {ROUTE_FN}(req) {{")), "{js}");
    assert!(js.contains("let req1_body;"), "{js}");
    assert!(js.contains("req1_body = req.body;"), "{js}");
    assert!(js.contains("return req1_body;"), "{js}");
}

#[test]
fn function_nodes_dispatch_through_the_function_table() {
    callable("js_greet", vec![Ty::Str], Ty::Str, |args| {
        Var::from(format!("hi {}", args[0].as_str().unwrap_or_default()))
    });
    let doc = doc_from(
        vec![
            node("req1", "web/request"),
            node_params(
                "get1",
                "data/get",
                Var::from(hyg::o! { "key": Var::from("who") }),
            ),
            node("fn1", "js_greet"),
            node("resp1", "web/respond"),
        ],
        vec![
            wire("req1", "exec", "fn1", "exec"),
            wire("fn1", "exec", "resp1", "exec"),
            wire("req1", "body", "get1", "value"),
            wire("get1", "out", "fn1", "in0"),
            wire("fn1", "out", "resp1", "body"),
        ],
        vec!["req1"],
    );
    let compiled = compile_doc(doc).expect("a function-node route converts");
    let js = &compiled.js;
    // the pure data/get inlines at its use site; the function node
    // dispatches through the host binding into the table
    assert!(
        js.contains(&format!(
            "{NODE_CALL_HOST}(\"js_greet\", JSON.stringify([req1_body[\"who\"]]))"
        )),
        "{js}"
    );
    assert!(js.contains("let fn1_out;"), "{js}");
    assert!(js.contains("return fn1_out;"), "{js}");
}

#[test]
fn branches_generate_if_else_and_literals_inline() {
    let doc = doc_from(
        vec![
            node("req1", "web/request"),
            node_params(
                "get_n",
                "data/get",
                Var::from(hyg::o! { "key": Var::from("n") }),
            ),
            node_params(
                "get_m",
                "data/get",
                Var::from(hyg::o! { "key": Var::from("m") }),
            ),
            node_params(
                "cmp1",
                "math/compare",
                Var::from(hyg::o! { "op": Var::from(">") }),
            ),
            node("branch1", "flow/branch"),
            node_params(
                "lit_big",
                "data/literal",
                Var::from(hyg::o! { "value": Var::from("big") }),
            ),
            node_params(
                "lit_small",
                "data/literal",
                Var::from(hyg::o! { "value": Var::from("small") }),
            ),
            node("then_resp", "web/respond"),
            node("else_resp", "web/respond"),
        ],
        vec![
            wire("req1", "exec", "branch1", "exec"),
            wire("req1", "body", "get_n", "value"),
            wire("req1", "body", "get_m", "value"),
            wire("get_n", "out", "cmp1", "a"),
            wire("get_m", "out", "cmp1", "b"),
            wire("cmp1", "result", "branch1", "condition"),
            wire("branch1", "then", "then_resp", "exec"),
            wire("branch1", "else", "else_resp", "exec"),
            wire("lit_big", "out", "then_resp", "body"),
            wire("lit_small", "out", "else_resp", "body"),
        ],
        vec!["req1"],
    );
    let compiled = compile_doc(doc).expect("a branching route converts");
    let js = &compiled.js;
    // pure nodes inline: the compare is one JS comparison, the literals
    // are JSON literals — and the two respond nodes are two returns
    assert!(
        js.contains("if ((req1_body[\"n\"] > req1_body[\"m\"])) {"),
        "{js}"
    );
    assert!(js.contains("return (\"big\");"), "{js}");
    assert!(js.contains("return (\"small\");"), "{js}");
}

#[test]
fn unwired_pins_read_undefined() {
    let doc = doc_from(
        vec![
            node("req1", "web/request"),
            node("branch1", "flow/branch"),
            node("resp1", "web/respond"),
        ],
        vec![
            wire("req1", "exec", "branch1", "exec"),
            wire("branch1", "then", "resp1", "exec"),
        ],
        vec!["req1"],
    );
    let compiled = compile_doc(doc).expect("unwired pins are legal (JS falsy rules)");
    let js = &compiled.js;
    assert!(js.contains("if (undefined) {"), "{js}");
    assert!(js.contains("return undefined;"), "{js}");
}

// ---- conversion failures carry graph coordinates ----

#[test]
fn unknown_kinds_fail_naming_the_node() {
    let doc = doc_from(
        vec![node("req1", "web/request"), node("bad1", "flow/frobnicate")],
        vec![wire("req1", "exec", "bad1", "exec")],
        vec!["req1"],
    );
    let err = compile_doc(doc).expect_err("unknown kind refuses");
    assert_eq!(err.node.as_deref(), Some("bad1"));
    assert!(err.to_string().contains("unknown node kind"), "{err}");

    // a callable name that is not in the table is the same failure —
    // function nodes are the function table's face
    let doc = doc_from(
        vec![node("req1", "web/request"), node("fn1", "ghost.callable")],
        vec![wire("req1", "exec", "fn1", "exec")],
        vec!["req1"],
    );
    let err = compile_doc(doc).expect_err("an unregistered callable refuses");
    assert!(err.to_string().contains("ghost.callable"), "{err}");
}

#[test]
fn wires_must_land_on_real_pins_of_matching_types() {
    callable("js_src_str", vec![], Ty::Str, |_| Var::from("src"));
    callable("js_dst_int", vec![Ty::I64], Ty::I64, |args| args[0].clone());

    // a pin the target doesn't have
    let doc = doc_from(
        vec![node("req1", "web/request"), node("fn1", "js_dst_int")],
        vec![
            wire("req1", "exec", "fn1", "exec"),
            wire("req1", "body", "fn1", "in9"),
        ],
        vec!["req1"],
    );
    let err = compile_doc(doc).expect_err("nonexistent in-pin refuses");
    assert_eq!(err.node.as_deref(), Some("fn1"));
    assert_eq!(err.pin.as_deref(), Some("in9"));
    assert!(err.to_string().contains("has no in-pin"), "{err}");

    // connected pins whose types disagree: Str out → int in
    let doc = doc_from(
        vec![
            node("req1", "web/request"),
            node("src1", "js_src_str"),
            node("dst1", "js_dst_int"),
        ],
        vec![
            wire("req1", "exec", "src1", "exec"),
            wire("src1", "exec", "dst1", "exec"),
            wire("src1", "out", "dst1", "in0"),
        ],
        vec!["req1"],
    );
    let err = compile_doc(doc).expect_err("type mismatch refuses");
    assert!(err.to_string().contains("pin type mismatch"), "{err}");
    assert!(err.to_string().contains("outputs string"), "{err}");
    assert!(err.to_string().contains("expects int"), "{err}");

    // exec and data do not mix
    let doc = doc_from(
        vec![node("req1", "web/request"), node("fn1", "js_dst_int")],
        vec![
            wire("req1", "exec", "fn1", "exec"),
            wire("req1", "body", "fn1", "exec"),
        ],
        vec!["req1"],
    );
    let err = compile_doc(doc).expect_err("exec/data mixing refuses");
    assert!(err.to_string().contains("do not mix"), "{err}");
}

#[test]
fn exec_cycles_fail_carrying_their_chain() {
    // the cycle needs nodes WITH exec outs — respond is terminal — so
    // two function nodes loop back on each other
    callable("js_cycle_a", vec![], Ty::Null, |_| Var::Null);
    callable("js_cycle_b", vec![], Ty::Null, |_| Var::Null);
    let doc = doc_from(
        vec![
            node("req1", "web/request"),
            node("a1", "js_cycle_a"),
            node("b1", "js_cycle_b"),
        ],
        vec![
            wire("req1", "exec", "a1", "exec"),
            wire("a1", "exec", "b1", "exec"),
            wire("b1", "exec", "a1", "exec"),
        ],
        vec!["req1"],
    );
    let err = compile_doc(doc).expect_err("an exec cycle has no generated form");
    assert!(err.to_string().contains("exec cycle"), "{err}");
    assert!(err.to_string().contains("a1 → b1 → a1"), "{err}");
    assert_eq!(
        err.flow,
        vec!["a1".to_string(), "b1".to_string(), "a1".to_string()],
        "the flow chain IS the cycle, walked from the entry"
    );
    assert_eq!(
        err.node.as_deref(),
        Some("a1"),
        "the revisit names where it closed"
    );
}

#[test]
fn data_cycles_fail_carrying_their_chain() {
    callable("js_greet_cycle", vec![Ty::Str], Ty::Str, |_| Var::from("x"));
    let doc = doc_from(
        vec![
            node("req1", "web/request"),
            node_params(
                "get1",
                "data/get",
                Var::from(hyg::o! { "key": Var::from("x") }),
            ),
            node_params(
                "cmp1",
                "math/compare",
                Var::from(hyg::o! { "op": Var::from(">") }),
            ),
            node("fn1", "js_greet_cycle"),
        ],
        vec![
            wire("req1", "exec", "fn1", "exec"),
            wire("req1", "body", "get1", "value"),
            wire("get1", "out", "cmp1", "a"),
            wire("cmp1", "result", "get1", "value"),
        ],
        vec!["req1"],
    );
    let err = compile_doc(doc).expect_err("a data cycle is the implicit-garbage footgun");
    assert!(err.to_string().contains("data cycle"), "{err}");
    assert_eq!(
        err.flow,
        vec!["get1".to_string(), "cmp1".to_string(), "get1".to_string()]
    );
}

#[test]
fn pure_node_params_are_validated_at_conversion() {
    // data/get without a key
    let doc = doc_from(
        vec![
            node("req1", "web/request"),
            node("get1", "data/get"),
            node("fn1", "js_greet_params"),
            node("resp1", "web/respond"),
        ],
        vec![
            wire("req1", "exec", "fn1", "exec"),
            wire("fn1", "exec", "resp1", "exec"),
            wire("req1", "body", "get1", "value"),
            wire("get1", "out", "fn1", "in0"),
        ],
        vec!["req1"],
    );
    callable("js_greet_params", vec![Ty::Str], Ty::Str, |_| {
        Var::from("x")
    });
    let err = compile_doc(doc).expect_err("data/get needs its key");
    assert!(err.to_string().contains("\"key\" param"), "{err}");

    // compare with a bogus op
    let doc = doc_from(
        vec![
            node("req1", "web/request"),
            node_params(
                "cmp1",
                "math/compare",
                Var::from(hyg::o! { "op": Var::from("=>>=") }),
            ),
            node("branch1", "flow/branch"),
        ],
        vec![
            wire("req1", "exec", "branch1", "exec"),
            wire("cmp1", "result", "branch1", "condition"),
        ],
        vec!["req1"],
    );
    let err = compile_doc(doc).expect_err("a bogus op refuses");
    assert!(err.to_string().contains("is not a comparison"), "{err}");

    // literal without a value
    let doc = doc_from(
        vec![
            node("req1", "web/request"),
            node("lit1", "data/literal"),
            node("resp1", "web/respond"),
        ],
        vec![
            wire("req1", "exec", "resp1", "exec"),
            wire("lit1", "out", "resp1", "body"),
        ],
        vec!["req1"],
    );
    let err = compile_doc(doc).expect_err("data/literal needs its value");
    assert!(err.to_string().contains("\"value\" param"), "{err}");
}

#[test]
fn entry_nodes_must_be_entry_capable() {
    let doc = doc_from(vec![node("resp1", "web/respond")], vec![], vec!["resp1"]);
    let err = compile_doc(doc).expect_err("a respond cannot start an execution path");
    assert!(
        err.to_string().contains("cannot start an execution path"),
        "{err}"
    );
}

// ---- the route end to end (script + web) ----

#[cfg(all(feature = "script", feature = "web"))]
mod live {
    use std::time::Duration;

    use hyg::dynamic::graph::route::GraphRoute;
    use hyg::web::Server;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use super::*;

    async fn http(addr: &str, raw: &str) -> (u16, String) {
        let mut s = TcpStream::connect(addr).await.expect("the server accepts");
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

    async fn boot(serv: &Server) -> String {
        let s = serv.clone();
        tokio::spawn(async move {
            let _ = s.start().await;
        });
        for _ in 0..100 {
            let addr = serv.local_addr();
            if !addr.is_empty() {
                return addr;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("server did not bind in time");
    }

    fn post(path: &str, body: &str) -> String {
        format!(
            "POST {path} HTTP/1.1\r\nhost: t\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_graph_serves_real_traffic() {
        callable("js_route_greet", vec![Ty::Str], Ty::Str, |args| {
            Var::from(format!("hello, {}!", args[0].as_str().unwrap_or_default()))
        });
        let doc = doc_from(
            vec![
                node("req1", "web/request"),
                node_params(
                    "get1",
                    "data/get",
                    Var::from(hyg::o! { "key": Var::from("who") }),
                ),
                node("greet1", "js_route_greet"),
                node("resp1", "web/respond"),
            ],
            vec![
                wire("req1", "exec", "greet1", "exec"),
                wire("greet1", "exec", "resp1", "exec"),
                wire("req1", "body", "get1", "value"),
                wire("get1", "out", "greet1", "in0"),
                wire("greet1", "out", "resp1", "body"),
            ],
            vec!["req1"],
        );
        let graph = Graph::from_var(&doc).expect("the greeting route is structurally valid");
        let route = GraphRoute::new(&graph).expect("the graph converts");
        let mut serv = Server::with_config(hyg::o! { "host": "127.0.0.1", "port": 0 });
        serv.post("/hello", route.handler());
        let addr = boot(&serv).await;

        // real traffic through generated JS: the function node dispatches
        // into the function table, the respond value becomes the body
        let (status, body) = http(&addr, &post("/hello", r#"{"who":"Rose"}"#)).await;
        assert_eq!((status, body.as_str()), (200, "hello, Rose!"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_branching_graph_routes_real_traffic() {
        let doc = doc_from(
            vec![
                node("req1", "web/request"),
                node_params(
                    "get_n",
                    "data/get",
                    Var::from(hyg::o! { "key": Var::from("n") }),
                ),
                node_params(
                    "get_m",
                    "data/get",
                    Var::from(hyg::o! { "key": Var::from("m") }),
                ),
                node_params(
                    "cmp1",
                    "math/compare",
                    Var::from(hyg::o! { "op": Var::from(">") }),
                ),
                node("branch1", "flow/branch"),
                node_params(
                    "lit_big",
                    "data/literal",
                    Var::from(hyg::o! { "value": Var::from("big") }),
                ),
                node_params(
                    "lit_small",
                    "data/literal",
                    Var::from(hyg::o! { "value": Var::from("small") }),
                ),
                node("then_resp", "web/respond"),
                node("else_resp", "web/respond"),
            ],
            vec![
                wire("req1", "exec", "branch1", "exec"),
                wire("req1", "body", "get_n", "value"),
                wire("req1", "body", "get_m", "value"),
                wire("get_n", "out", "cmp1", "a"),
                wire("get_m", "out", "cmp1", "b"),
                wire("cmp1", "result", "branch1", "condition"),
                wire("branch1", "then", "then_resp", "exec"),
                wire("branch1", "else", "else_resp", "exec"),
                wire("lit_big", "out", "then_resp", "body"),
                wire("lit_small", "out", "else_resp", "body"),
            ],
            vec!["req1"],
        );
        let graph = Graph::from_var(&doc).expect("the branch route is structurally valid");
        let route = GraphRoute::new(&graph).expect("the graph converts");
        let mut serv = Server::with_config(hyg::o! { "host": "127.0.0.1", "port": 0 });
        serv.post("/size", route.handler());
        let addr = boot(&serv).await;

        // both sides of the branch, driven by real request bodies — the
        // pure compare inlined, the literals as return values
        let (status, body) = http(&addr, &post("/size", r#"{"n":5,"m":3}"#)).await;
        assert_eq!((status, body.as_str()), (200, "big"));
        let (status, body) = http(&addr, &post("/size", r#"{"n":1,"m":9}"#)).await;
        assert_eq!((status, body.as_str()), (200, "small"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failing_node_fails_the_request_loud() {
        failing_callable("js_route_boom", vec![], Ty::Null);
        let doc = doc_from(
            vec![
                node("req1", "web/request"),
                node("boom1", "js_route_boom"),
                node("resp1", "web/respond"),
            ],
            vec![
                wire("req1", "exec", "boom1", "exec"),
                wire("boom1", "exec", "resp1", "exec"),
            ],
            vec!["req1"],
        );
        let graph = Graph::from_var(&doc).expect("structurally valid");
        let route = GraphRoute::new(&graph).expect("converts");
        let mut serv = Server::with_config(hyg::o! { "host": "127.0.0.1", "port": 0 });
        serv.post("/boom", route.handler());
        let addr = boot(&serv).await;

        // the route still serves (the graph converted and materialized);
        // the failing node call surfaces as a 500 with the message —
        // loud, never silent
        let (status, body) = http(&addr, &post("/boom", "{}")).await;
        assert_eq!(status, 500, "a failing node call is a loud 500");
        assert!(body.contains("js_route_boom exploded"), "{body}");
    }
}
