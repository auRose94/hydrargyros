//! The graph-document model: `.json.hyg` round-trips and structural
//! validation. Feature-free (the model is pure data), so these run in the
//! core suite. The document here is the design sketch's shape, corrected
//! to the landed wire schema (`from_pin`/`to_pin` — the sketch's duplicate
//! `"pin"` keys are unrepresentable in JSON).

use std::fs;

use hyg::dynamic::graph::model::{Graph, GRAPH_FORMAT};
use hyg::Var;

/// The sketch-shaped document: an order-flow graph — request → branch →
/// respond, with positions and params riding along.
fn order_flow() -> Var {
    hyg::Var::from(hyg::o! {
        "graph": Var::from("order-flow"),
        "format": Var::from(GRAPH_FORMAT),
        "nodes": Var::from(hyg::a![
            Var::from(hyg::o! {
                "id": Var::from("req1"),
                "type": Var::from("web/request"),
                "pos": Var::from(hyg::a![Var::from(80i64), Var::from(120i64)]),
            }),
            Var::from(hyg::o! {
                "id": Var::from("branch1"),
                "type": Var::from("flow/branch"),
                "pos": Var::from(hyg::a![Var::from(320i64), Var::from(140i64)]),
                "params": Var::from(hyg::o! { "threshold": Var::from(100i64) }),
            }),
            Var::from(hyg::o! {
                "id": Var::from("resp1"),
                "type": Var::from("web/respond"),
                "pos": Var::from(hyg::a![Var::from(560i64), Var::from(110i64)]),
            }),
        ]),
        "wires": Var::from(hyg::a![
            Var::from(hyg::o! {
                "from": Var::from("req1"),
                "from_pin": Var::from("exec"),
                "to": Var::from("branch1"),
                "to_pin": Var::from("exec_in"),
            }),
            Var::from(hyg::o! {
                "from": Var::from("branch1"),
                "from_pin": Var::from("then"),
                "to": Var::from("resp1"),
                "to_pin": Var::from("exec_in"),
            }),
        ]),
        "entry": Var::from(hyg::a![Var::from("req1")]),
    })
}

#[test]
fn graph_var_round_trips_through_json_text() {
    // the carrier contract: Var → JSON text → Var is identity, twice over
    // (insertion order survives Var's serde — objects are ordered maps)
    let doc = order_flow();
    let text = serde_json::to_string(&doc).expect("serializes");
    let back: Var = serde_json::from_str(&text).expect("parses");
    assert_eq!(back, doc, "text → Var → text preserves the document");
    let text2 = serde_json::to_string(&back).expect("serializes again");
    assert_eq!(text2, text, "the round trip is stable");
}

#[test]
fn graph_var_round_trips_through_a_json_hyg_file() {
    // the .hyg carrier is a FILE convention: graph.json.hyg
    let dir = tempfile::tempdir().expect("scratch dir");
    let path = dir.path().join("graph.json.hyg");
    let doc = order_flow();
    fs::write(&path, serde_json::to_string(&doc).expect("serializes")).expect("writes");

    let loaded: Var =
        serde_json::from_str(&fs::read_to_string(&path).expect("reads")).expect("parses");
    let graph = Graph::from_var(&loaded).expect("a real .json.hyg file validates");
    assert_eq!(graph.name, "order-flow");
    assert_eq!(graph.nodes.len(), 3);
    assert_eq!(graph.entry, vec!["req1".to_string()]);
}

#[test]
fn model_parses_and_regenerates_canonically() {
    let doc = order_flow();
    let graph = Graph::from_var(&doc).expect("the sketch-shaped document validates");

    // the typed view
    assert_eq!(graph.name, "order-flow");
    assert_eq!(graph.nodes.len(), 3);
    let branch = graph.node("branch1").expect("branch1 exists");
    assert_eq!(branch.kind, "flow/branch");
    assert_eq!(branch.pos_xy(), Some((320.0, 140.0)));
    assert_eq!(
        branch.params.get("threshold"),
        Var::from(100i64),
        "params ride raw"
    );
    assert_eq!(graph.wires.len(), 2);
    assert_eq!(graph.wires[0].from_pin, "exec");
    assert_eq!(graph.wires[0].to_pin, "exec_in");
    assert_eq!(graph.entry, vec!["req1".to_string()]);

    // canonical regeneration: schema fields, sketch order, document order
    assert_eq!(graph.to_var(), doc, "to_var rebuilds the document");
}

#[test]
fn wire_endpoints_and_entry_must_land_on_known_nodes() {
    let doc = order_flow();
    // a wire into a node that does not exist
    doc.get("wires")
        .as_list()
        .expect("wires list")
        .push(Var::from(hyg::o! {
            "from": Var::from("branch1"),
            "from_pin": Var::from("else"),
            "to": Var::from("resp9"),
            "to_pin": Var::from("exec_in"),
        }));
    let err = Graph::from_var(&doc).expect_err("unknown wire endpoint fails");
    assert_eq!(
        err.node.as_deref(),
        Some("resp9"),
        "the error names the node to click"
    );
    assert!(err.to_string().contains("unknown node"), "{err}");
    assert!(err.flow.is_empty(), "structural errors carry no flow chain");

    // an entry naming a node that does not exist
    let doc = order_flow();
    doc.get("entry")
        .as_list()
        .expect("entry list")
        .push(Var::from("ghost"));
    let err = Graph::from_var(&doc).expect_err("unknown entry fails");
    assert!(err.to_string().contains("ghost"), "{err}");
}

#[test]
fn node_ids_are_unique_identifiers() {
    // duplicate id
    let doc = order_flow();
    doc.get("nodes")
        .as_list()
        .expect("nodes list")
        .push(Var::from(hyg::o! {
            "id": Var::from("req1"),
            "type": Var::from("web/request"),
        }));
    let err = Graph::from_var(&doc).expect_err("duplicate id fails");
    assert_eq!(
        err.node.as_deref(),
        Some("req1"),
        "the error names the duplicated id"
    );
    assert!(err.to_string().contains("duplicate node id"), "{err}");

    // a non-identifier id (ids name paths, errors, and generated code)
    let doc = order_flow();
    doc.get("nodes")
        .as_list()
        .expect("nodes list")
        .push(Var::from(hyg::o! {
            "id": Var::from("9lives"),
            "type": Var::from("flow/branch"),
        }));
    let err = Graph::from_var(&doc).expect_err("non-identifier id fails");
    assert!(err.to_string().contains("9lives"), "{err}");
}

#[test]
fn pins_must_be_named_and_identifiers() {
    let doc = order_flow();
    // missing pin name
    doc.get("wires")
        .as_list()
        .expect("wires list")
        .push(Var::from(hyg::o! {
            "from": Var::from("req1"),
            "from_pin": Var::from("exec"),
            "to": Var::from("resp1"),
        }));
    let err = Graph::from_var(&doc).expect_err("missing to_pin fails");
    assert!(err.to_string().contains("to_pin"), "{err}");

    // an invalid pin name
    let doc = order_flow();
    doc.get("wires")
        .as_list()
        .expect("wires list")
        .push(Var::from(hyg::o! {
            "from": Var::from("req1"),
            "from_pin": Var::from("has space"),
            "to": Var::from("resp1"),
            "to_pin": Var::from("exec_in"),
        }));
    let err = Graph::from_var(&doc).expect_err("invalid pin name fails");
    assert!(err.to_string().contains("from_pin"), "{err}");
}

#[test]
fn the_format_gate_is_explicit() {
    let doc = order_flow();
    doc.as_object()
        .expect("object")
        .set("format", Var::from(2i64));
    let err = Graph::from_var(&doc).expect_err("wrong format fails");
    assert!(
        err.to_string().contains("format 1"),
        "the error names what this hyg speaks: {err}"
    );

    let doc = order_flow();
    doc.as_object()
        .expect("object")
        .set("format", Var::from("one"));
    let err = Graph::from_var(&doc).expect_err("non-integer format fails");
    assert!(err.to_string().contains("format"), "{err}");
}

#[test]
fn shape_failures_carry_the_path_voice() {
    // not an object at all
    let err = Graph::from_var(&Var::from("hello")).expect_err("non-object fails");
    assert!(err.to_string().contains("expected an object"), "{err}");

    // missing name
    let doc = Var::from(hyg::o! { "format": Var::from(GRAPH_FORMAT) });
    let err = Graph::from_var(&doc).expect_err("missing name fails");
    assert!(err.to_string().contains("\"graph\""), "{err}");

    // a node that is not an object — the error indexes it (nodes[2])
    let doc = order_flow();
    doc.get("nodes")
        .as_list()
        .expect("nodes list")
        .push(Var::from("not a node"));
    let err = Graph::from_var(&doc).expect_err("non-object node fails");
    assert!(err.to_string().contains("nodes[3]"), "{err}");
}

#[test]
fn structural_tolerances_are_deliberate() {
    // fan-out: one exec pin feeding two targets is legal
    let fanout = Var::from(hyg::o! {
        "graph": Var::from("fanout"),
        "format": Var::from(GRAPH_FORMAT),
        "nodes": Var::from(hyg::a![
            Var::from(hyg::o! { "id": Var::from("src"), "type": Var::from("flow/seq") }),
            Var::from(hyg::o! { "id": Var::from("a"), "type": Var::from("flow/log") }),
            Var::from(hyg::o! { "id": Var::from("b"), "type": Var::from("flow/log") }),
        ]),
        "wires": Var::from(hyg::a![
            Var::from(hyg::o! {
                "from": Var::from("src"),
                "from_pin": Var::from("exec"),
                "to": Var::from("a"),
                "to_pin": Var::from("exec_in"),
            }),
            Var::from(hyg::o! {
                "from": Var::from("src"),
                "from_pin": Var::from("exec"),
                "to": Var::from("b"),
                "to_pin": Var::from("exec_in"),
            }),
        ]),
        "entry": Var::from(hyg::a![Var::from("src")]),
    });
    let graph = Graph::from_var(&fanout).expect("fan-out is legal");
    assert_eq!(graph.wires.len(), 2);

    // cycles are legal — loops are real
    let cycle = Var::from(hyg::o! {
        "graph": Var::from("loop"),
        "format": Var::from(GRAPH_FORMAT),
        "nodes": Var::from(hyg::a![
            Var::from(hyg::o! { "id": Var::from("a"), "type": Var::from("flow/while") }),
            Var::from(hyg::o! { "id": Var::from("b"), "type": Var::from("flow/body") }),
        ]),
        "wires": Var::from(hyg::a![
            Var::from(hyg::o! {
                "from": Var::from("a"),
                "from_pin": Var::from("body"),
                "to": Var::from("b"),
                "to_pin": Var::from("exec_in"),
            }),
            Var::from(hyg::o! {
                "from": Var::from("b"),
                "from_pin": Var::from("exec"),
                "to": Var::from("a"),
                "to_pin": Var::from("exec_in"),
            }),
        ]),
        "entry": Var::from(hyg::a![Var::from("a")]),
    });
    Graph::from_var(&cycle).expect("cycles are legal");

    // a bare document is a valid empty graph; sub-graphs have no entry
    let bare = Var::from(hyg::o! {
        "graph": Var::from("empty"),
        "format": Var::from(GRAPH_FORMAT),
    });
    let graph = Graph::from_var(&bare).expect("a bare document is a valid empty graph");
    assert!(graph.nodes.is_empty() && graph.wires.is_empty() && graph.entry.is_empty());
    Graph::from_var(&order_flow_regenerated_without_entry()).expect("empty entry is a sub-graph");

    // exact duplicate wires are noise, not fan-out
    let doc = order_flow();
    let wire = doc.get("wires").as_list().expect("wires list").iter()[0].clone();
    doc.get("wires").as_list().expect("wires list").push(wire);
    let err = Graph::from_var(&doc).expect_err("duplicate wire fails");
    assert!(err.to_string().contains("duplicate wire"), "{err}");
    assert!(err.to_string().contains("fan-out is legal"), "{err}");
}

/// The order-flow document with its entry list removed — the sub-graph
/// shape (called by a function node, never launched directly).
fn order_flow_regenerated_without_entry() -> Var {
    let doc = order_flow();
    let obj = doc.as_object().expect("object");
    let rebuilt = hyg::o! {};
    for (k, v) in obj.entries() {
        if k != "entry" {
            rebuilt.set(&k, v);
        }
    }
    Var::from(rebuilt)
}
