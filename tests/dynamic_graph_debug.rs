//! The graph debugger (M5): breakpoints on nodes, watches on pins,
//! single-stepping over exec wires — one implementation for both
//! modules. The feature-free tests pin the interpreter's contract; the
//! game test steps a graph through the REAL pump (one node per frame);
//! the gated tests prove the JS path and the interpreter agree, and
//! that a live route parks mid-flight with its staged request visible.
//!
//! Convention: any test whose run can sit parked in `run_blocking`
//! holds a [`RunGuard`] — a failed assertion must abort the run
//! (unwinding the parked thread), never strand it: a stranded runner
//! freezes a tokio worker and the whole test binary hangs in silence.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hyg::dynamic::functions::{register_function, FunctionMeta};
use hyg::dynamic::graph::debug::{
    active_runs, register_run, unregister_run, DebugEvent, DebugRun, PauseReason,
};
use hyg::dynamic::graph::inspect::inspect;
use hyg::dynamic::graph::model::{Graph, GRAPH_FORMAT};
use hyg::var::Ty;
use hyg::Var;

// ---- the demo graph: request → get(who) → greet function node → respond ----

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

fn greeting_graph() -> Graph {
    let doc = Var::from(hyg::o! {
        "graph": Var::from("debug-greeting"),
        "format": Var::from(GRAPH_FORMAT),
        "nodes": Var::from(hyg::a![
            node("req1", "web/request"),
            node_params("get1", "data/get", Var::from(hyg::o! { "key": Var::from("who") })),
            node("greet1", "dbg_greet"),
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
    });
    Graph::from_var(&doc).expect("structurally valid")
}

fn demo_input() -> Var {
    Var::from(hyg::o! {
        "params": Var::from(hyg::o! {}),
        "query": Var::from(hyg::o! {}),
        "headers": Var::from(hyg::o! {}),
        "body": Var::from(hyg::o! { "who": Var::from("Rose") }),
    })
}

fn register_greet() {
    register_function(
        "dbg_greet",
        FunctionMeta {
            summary: "greets the caller".into(),
            inputs: vec![Ty::Str],
            output: Ty::Str,
            origin: "host:dbg_greet".into(),
        },
        Arc::new(Mutex::new(Box::new(|args: &[Var]| {
            Ok(Var::from(format!(
                "hello, {}!",
                args[0].as_str().unwrap_or_default()
            )))
        }))),
    );
}

/// Aborts the run when dropped — the anti-hang guard. A run parked in
/// `run_blocking` is woken ONLY by a command (`resume`/`step`/`abort`),
/// and the parked thread holds a tokio worker: if a test fails between
/// the pause and the command, the stranded worker freezes the runtime,
/// the harness's buffered output never flushes, and the binary hangs in
/// silence forever. Drop runs during panic unwind, so holding a guard
/// converts every failure path into a visible test failure instead.
struct RunGuard(Arc<DebugRun>);

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

// ---- breakpoints, watches, resume ----

#[test]
fn breakpoints_pause_and_watches_read_live_values() {
    register_greet();
    let run = DebugRun::start(&greeting_graph(), demo_input(), vec!["greet1".into()])
        .expect("the graph validates");

    // one drive runs the unattended segment and stops at the breakpoint —
    // BEFORE the paused node executes
    match run.drive().expect("drives") {
        hyg::dynamic::graph::debug::DriveOutcome::Paused { node, reason } => {
            assert_eq!(node, "greet1");
            assert_eq!(reason, PauseReason::Breakpoint);
        }
        other => panic!("expected a pause, got {other:?}"),
    }

    // the staged request is visible; the walked path EXCLUDES the paused
    // node (it has not executed)
    assert_eq!(run.input().get("body").get("who").as_str(), Some("Rose"));
    assert_eq!(run.path(), vec!["req1".to_string()]);

    // watches: the function node's input pin resolves its pure chain on
    // demand; the request node's out reads the store
    assert_eq!(
        run.watch("greet1", "in0").expect("watches the in-pin"),
        Var::from("Rose")
    );
    assert_eq!(
        run.watch("req1", "body")
            .expect("watches the request's body")
            .get("who")
            .as_str(),
        Some("Rose")
    );
    // an unexecuted node's out is an explicit error, never a silent Null
    let err = run
        .watch("greet1", "out")
        .expect_err("the paused node has not produced its out yet");
    assert!(err.to_string().contains("has not executed"), "{err}");

    // resume → the run completes through the SAME function table
    assert!(run.resume());
    match run.drive().expect("drives to the finish") {
        hyg::dynamic::graph::debug::DriveOutcome::Finished { result } => {
            assert_eq!(result.expect("succeeds").as_str(), Some("hello, Rose!"));
        }
        other => panic!("expected the finish, got {other:?}"),
    }
    // the walked path is now the whole chain
    assert_eq!(run.path(), vec!["req1", "greet1", "resp1"]);
}

#[test]
fn stepping_advances_one_node_at_a_time() {
    register_greet();
    let run = DebugRun::start_stepping(&greeting_graph(), demo_input(), vec![])
        .expect("the graph validates");

    let mut order: Vec<String> = Vec::new();
    loop {
        match run.drive().expect("drives") {
            hyg::dynamic::graph::debug::DriveOutcome::Paused { node, reason } => {
                assert_eq!(
                    reason,
                    PauseReason::Step,
                    "stepping pauses, not breakpoints"
                );
                order.push(node);
                assert!(run.step(), "a paused run steps");
            }
            hyg::dynamic::graph::debug::DriveOutcome::Finished { result } => {
                assert_eq!(result.expect("succeeds").as_str(), Some("hello, Rose!"));
                break;
            }
        }
    }
    // exactly the exec chain, one node per step — pure nodes inline and
    // never appear (get1 computes on demand)
    assert_eq!(order, vec!["req1", "greet1", "resp1"]);
}

#[test]
fn a_failing_node_finishes_with_the_walked_chain() {
    register_function(
        "dbg_boom",
        FunctionMeta {
            summary: String::new(),
            inputs: vec![],
            output: Ty::Null,
            origin: "host:dbg_boom".into(),
        },
        Arc::new(Mutex::new(Box::new(
            move |_args: &[Var]| -> Result<Var, hyg::dynamic::engine::ScriptError> {
                Err(hyg::dynamic::engine::ScriptError::Engine("boom".into()))
            },
        ))),
    );
    let doc = Var::from(hyg::o! {
        "graph": Var::from("boom-graph"),
        "format": Var::from(GRAPH_FORMAT),
        "nodes": Var::from(hyg::a![
            node("req1", "web/request"),
            node("boom1", "dbg_boom"),
            node("resp1", "web/respond"),
        ]),
        "wires": Var::from(hyg::a![
            wire("req1", "exec", "boom1", "exec"),
            wire("boom1", "exec", "resp1", "exec"),
        ]),
        "entry": Var::from(hyg::a![Var::from("req1")]),
    });
    let graph = Graph::from_var(&doc).expect("structurally valid");
    let run = DebugRun::start(&graph, demo_input(), vec![]).expect("validates");

    match run.drive().expect("drives") {
        hyg::dynamic::graph::debug::DriveOutcome::Finished { result } => {
            let err = result.expect_err("the failing node finishes the run with an error");
            assert!(err.to_string().contains("boom"), "{err}");
            // the flow chain is the path ACTUALLY walked, failure first —
            // ground truth, not a prediction
            assert_eq!(err.flow, vec!["boom1".to_string(), "req1".to_string()]);
            assert_eq!(err.node.as_deref(), Some("boom1"));
        }
        other => panic!("expected the finish, got {other:?}"),
    }
}

#[test]
fn abort_unwedges_a_stuck_run() {
    register_greet();
    let run = DebugRun::start(&greeting_graph(), demo_input(), vec!["greet1".into()])
        .expect("the graph validates");
    let _guard = RunGuard(Arc::clone(&run));
    let _ = run.drive().expect("drives to the breakpoint");

    // the debugger side observes the pause, then aborts instead of
    // resuming — a stuck run must not hold a frame or a request forever
    match run.wait_event(Duration::from_secs(1)) {
        Some(DebugEvent::Paused { node, .. }) => assert_eq!(node, "greet1"),
        other => panic!("expected the pause event, got {other:?}"),
    }
    run.abort();
    match run.wait_event(Duration::from_secs(1)) {
        Some(DebugEvent::Finished { result }) => {
            assert!(result.is_err(), "the aborted run finishes with an error");
        }
        other => panic!("expected the finish event, got {other:?}"),
    }
    assert!(
        run.run_blocking().is_err(),
        "the executing side unwinds too"
    );
}

#[test]
fn the_registry_is_the_attach_seam() {
    register_greet();
    let run = DebugRun::start(&greeting_graph(), demo_input(), vec![]).expect("validates");
    let id = register_run(Arc::clone(&run));
    let found = active_runs().into_iter().find(|(rid, _)| *rid == id);
    assert!(found.is_some(), "the active run is findable");
    let (_, found) = found.expect("checked above");
    assert!(Arc::ptr_eq(&run, &found), "the same run");
    unregister_run(id);
    assert!(
        !active_runs().iter().any(|(rid, _)| *rid == id),
        "a finished run is gone"
    );
}

// ---- the inspector: descriptors with resolving doc links ----

#[test]
fn the_inspector_describes_nodes_and_links_resolve() {
    register_greet();
    let graph = greeting_graph();
    let doc = inspect(&graph);

    assert_eq!(doc.get("kind").as_str(), Some("inspect"));
    let nodes = doc.get("nodes").as_list().expect("nodes list").iter();
    let by_id = |id: &str| {
        nodes
            .iter()
            .find(|n| n.get("id").as_str() == Some(id))
            .expect("the node is described")
            .clone()
    };
    let _ = &nodes;

    // built-ins: summary + doc link that resolves to a real file
    let req = by_id("req1");
    assert_eq!(
        req.get("summary").as_str(),
        Some("the route's entry — the staged request")
    );
    let doc_var = req.get("doc");
    let doc_link = doc_var
        .as_str()
        .expect("built-ins carry doc links")
        .to_string();
    let resolved = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(&doc_link);
    assert!(resolved.exists(), "the doc link resolves: {doc_link}");

    // pins carry name, type, and doc
    let out_pins = req.get("out").as_list().expect("out pins").iter();
    let body_pin = out_pins
        .iter()
        .find(|p| p.get("pin").as_str() == Some("body"))
        .expect("the body pin is described")
        .clone();
    assert!(
        matches!(body_pin.get("ty"), Var::Null),
        "body is the untyped marker"
    );
    assert!(body_pin
        .get("doc")
        .as_str()
        .unwrap_or_default()
        .contains("body"));

    // function nodes surface the host function behind them
    let greet = by_id("greet1");
    assert_eq!(greet.get("origin").as_str(), Some("host:dbg_greet"));
    assert_eq!(greet.get("summary").as_str(), Some("greets the caller"));
    assert!(
        matches!(greet.get("doc"), Var::Null),
        "function nodes' docs live with their origin"
    );
    let in_pins = greet.get("in").as_list().expect("in pins").iter();
    let in0 = in_pins
        .iter()
        .find(|p| p.get("pin").as_str() == Some("in0"))
        .expect("in0 is described")
        .clone();
    assert_eq!(
        in0.get("ty").as_str(),
        Some("string"),
        "the callable's signature"
    );

    // a graph with an unresolvable kind shows the problem, not a crash
    let broken = Var::from(hyg::o! {
        "graph": Var::from("broken"),
        "format": Var::from(GRAPH_FORMAT),
        "nodes": Var::from(hyg::a![node("x1", "ghost/kind")]),
        "wires": Var::from(hyg::a![]),
        "entry": Var::from(hyg::a![]),
    });
    let graph = Graph::from_var(&broken).expect("the model tolerates unknown kinds");
    let doc = inspect(&graph);
    let x1 = doc
        .get("nodes")
        .as_list()
        .expect("nodes")
        .iter()
        .first()
        .expect("the node")
        .clone();
    assert!(
        x1.get("summary")
            .as_str()
            .unwrap_or_default()
            .contains("unknown node kind"),
        "{x1}"
    );
}

// ---- the game frame: one node per frame through the REAL pump ----

#[test]
fn the_pump_steps_a_graph_one_node_per_frame() {
    use hyg::game::{component::Priority, Engine, Entity};

    register_greet();
    let run = DebugRun::start_stepping(&greeting_graph(), demo_input(), vec![])
        .expect("the graph validates");
    let finished = Arc::new(AtomicBool::new(false));
    let done_flag = Arc::clone(&finished);

    /// The gameplay component shape: a graph drives its logic per frame —
    /// one node per update, paused between frames, no blocking.
    struct GraphStepper {
        run: Arc<DebugRun>,
        done: Arc<AtomicBool>,
    }
    impl hyg::game::Component for GraphStepper {
        fn priority(&self) -> Priority {
            Priority::High
        }
        fn update(&mut self, _dt: f64) {
            if self.run.is_paused() {
                self.run.step();
            }
            if let Ok(hyg::dynamic::graph::debug::DriveOutcome::Finished { result }) =
                self.run.drive()
            {
                self.done.store(result.is_ok(), Ordering::Relaxed);
            }
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    let mut engine = Engine::new("acme", "graph-frame-test");
    let mut entity = Entity::new("graph-runner");
    entity.add_component(Box::new(GraphStepper {
        run: Arc::clone(&run),
        done: done_flag,
    }));
    engine.add_entity(entity);

    // one node per frame: the pump IS the debugger's pacing
    for _ in 0..4 {
        engine.update(0.016);
    }
    assert!(
        finished.load(Ordering::Relaxed),
        "the graph finished across frames"
    );
    assert_eq!(
        run.path(),
        vec!["req1", "greet1", "resp1"],
        "the whole exec chain walked"
    );
}

// ---- the JS path and the interpreter agree (script) ----

#[cfg(feature = "script")]
mod agreement {
    use hyg::dynamic::graph::debug::DebugRun;
    use hyg::dynamic::graph::to_js::{compile, NODE_CALL_HOST, ROUTE_FN};
    use hyg::script::Script;

    use super::*;

    #[test]
    fn the_interpreter_and_the_js_path_agree() {
        register_greet();
        let graph = greeting_graph();
        let input = demo_input();

        // the interpreter
        let run = DebugRun::start(&graph, input.clone(), vec![]).expect("validates");
        let via_interpreter = run.run_blocking().expect("the run completes");

        // the JS path — the same graph compiled, the same function table
        let compiled = compile(&graph).expect("the JS path compiles");
        let mut script = Script::new(&compiled.js).expect("the generated JS parses");
        script.register(NODE_CALL_HOST, |args: &[Var]| {
            let name = args.first().and_then(Var::as_str).unwrap_or_default();
            let args_json = args.get(1).and_then(Var::as_str).unwrap_or("[]");
            let vars: Vec<Var> = serde_json::from_str(args_json).unwrap_or_default();
            hyg::dynamic::functions::call_function(name, &vars).unwrap_or_else(|_| Var::Null)
        });
        let via_js = script.call(ROUTE_FN, &[input]).expect("the JS path serves");

        // two compilers, two execution engines, ONE answer
        assert_eq!(via_js, via_interpreter);
        assert_eq!(via_interpreter.as_str(), Some("hello, Rose!"));
    }
}

// ---- the live route parks mid-flight (script + web) ----

#[cfg(all(feature = "script", feature = "web"))]
mod live_route {
    use std::time::Duration;

    use hyg::dynamic::graph::debug::DriveOutcome;
    use hyg::dynamic::graph::route::GraphRoute;
    use hyg::web::Server;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use super::*;

    async fn post(addr: &str, path: &str, body: &str) -> (u16, String) {
        let mut s = TcpStream::connect(addr).await.expect("the server accepts");
        let raw = format!(
            "POST {path} HTTP/1.1\r\nhost: t\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(raw.as_bytes()).await.expect("request writes");
        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut buf))
            .await
            .expect("the response arrives in time (after resume)")
            .expect("the response reads");
        let text = String::from_utf8_lossy(&buf).into_owned();
        let (head, resp_body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (status, resp_body.to_string())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_live_route_parks_at_a_breakpoint() {
        register_greet();
        let graph = greeting_graph();
        let route = GraphRoute::new(&graph).expect("the route builds");

        let mut serv = Server::with_config(hyg::o! { "host": "127.0.0.1", "port": 0 });
        serv.post("/dbg", route.debug_handler(vec!["greet1".into()]));
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

        // real traffic — the request parks mid-flight at the breakpoint
        let client = tokio::spawn(async move { post(&addr, "/dbg", r#"{"who":"Rose"}"#).await });

        // the debugger client polls the registry and attaches — matching
        // THIS run by its staged input: the registry is process-global and
        // this binary's other tests register runs concurrently, so a bare
        // "first run" grab can steal a foreign one
        let mut run = None;
        for _ in 0..500 {
            for (_, r) in active_runs() {
                if r.input().get("body").get("who").as_str() == Some("Rose") {
                    run = Some(r);
                    break;
                }
            }
            if run.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let run = run.expect("the debug run is registered while the request is parked");
        // any failure from here on must abort the parked runner — the
        // guard unwinds it even through a panic (see RunGuard)
        let _guard = RunGuard(Arc::clone(&run));

        // the pause event, the staged request visible at the breakpoint
        match run.wait_event(Duration::from_secs(5)) {
            Some(DebugEvent::Paused { node, reason }) => {
                assert_eq!(node, "greet1");
                assert_eq!(reason, PauseReason::Breakpoint);
            }
            other => panic!("expected the pause, got {other:?}"),
        }
        assert_eq!(
            run.watch("greet1", "in0")
                .expect("watches the pinned input"),
            Var::from("Rose"),
            "the staged request is visible mid-flight"
        );

        // the response has NOT completed while parked
        let mut client = client;
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut client)
                .await
                .is_err(),
            "the request is frozen at the breakpoint"
        );

        // resume → the request completes, indistinguishable from the
        // compiled path's response
        assert!(run.resume());
        let (status, body) = client.await.expect("the client joins");
        assert_eq!((status, body.as_str()), (200, "hello, Rose!"));
        assert!(!run.is_paused(), "the run is done");
        match run.drive() {
            Ok(DriveOutcome::Finished { result }) => {
                assert_eq!(result.expect("succeeds").as_str(), Some("hello, Rose!"));
            }
            other => panic!("expected the finish, got {other:?}"),
        }
    }
}
