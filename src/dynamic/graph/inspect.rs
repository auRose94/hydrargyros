//! The graph inspector — every node's descriptor as a `.hyg` document:
//! kind, summary, pins with docs, and doc links that resolve to real
//! files. The editor's hover text and the inspector panel read this;
//! function nodes surface the host function behind them (origin +
//! summary from the callable table). Pure data, no feature gate.

use crate::dynamic::functions::function_meta;
use crate::dynamic::graph::model::Graph;
use crate::dynamic::graph::spec::spec_for;
use crate::var::{Ty, Var};

/// The inspector document's kind marker.
pub const INSPECT_KIND: &str = "inspect";

/// The inspector document for a graph — a serialized-Var-ready
/// descriptor of every node. A kind that no longer resolves (a callable
/// unregistered since the graph was saved) still appears, marked
/// unknown: the inspector shows the problem instead of hiding it.
#[must_use]
pub fn inspect(graph: &Graph) -> Var {
    let nodes: Vec<Var> = graph
        .nodes
        .iter()
        .map(|node| match spec_for(&node.kind) {
            Some(spec) => {
                let origin = function_meta(&node.kind)
                    .map(|meta| Var::from(meta.origin.as_str()))
                    .unwrap_or(Var::Null);
                let summary = if function_meta(&node.kind).is_some_and(|m| !m.summary.is_empty()) {
                    Var::from(
                        function_meta(&node.kind)
                            .expect("checked above")
                            .summary
                            .as_str(),
                    )
                } else {
                    Var::from(spec.summary.as_str())
                };
                Var::from(crate::o! {
                    "id": Var::from(node.id.as_str()),
                    "kind": Var::from(node.kind.as_str()),
                    "summary": summary,
                    "doc": doc_var(&spec.doc),
                    "origin": origin,
                    "pure": Var::from(spec.pure),
                    "exec_in": Var::from(spec.exec_in),
                    "exec_outs": Var::from(list_of(&spec.exec_outs)),
                    "in": Var::from(pins_var(&spec.data_ins)),
                    "out": Var::from(pins_var(&spec.data_outs)),
                })
            }
            None => Var::from(crate::o! {
                "id": Var::from(node.id.as_str()),
                "kind": Var::from(node.kind.as_str()),
                "summary": Var::from(format!("unknown node kind {:?}", node.kind)),
                "doc": Var::Null,
                "origin": Var::Null,
                "pure": Var::from(false),
                "exec_in": Var::from(false),
                "exec_outs": Var::from(crate::a![]),
                "in": Var::from(crate::a![]),
                "out": Var::from(crate::a![]),
            }),
        })
        .collect();
    let entry: Vec<Var> = graph
        .entry
        .iter()
        .map(|id| Var::from(id.as_str()))
        .collect();
    Var::from(crate::o! {
        "graph": Var::from(graph.name.as_str()),
        "format": Var::from(1i64),
        "kind": Var::from(INSPECT_KIND),
        "nodes": Var::from(crate::List::from_iter(nodes)),
        "entry": Var::from(crate::List::from_iter(entry)),
    })
}

fn doc_var(doc: &Option<String>) -> Var {
    match doc {
        Some(path) => Var::from(path.as_str()),
        None => Var::Null,
    }
}

fn list_of(items: &[String]) -> crate::List {
    items.iter().map(|s| Var::from(s.as_str())).collect()
}

fn pins_var(pins: &[crate::dynamic::graph::spec::PinSpec]) -> crate::List {
    pins.iter()
        .map(|pin| {
            Var::from(crate::o! {
                "pin": Var::from(pin.name.as_str()),
                "ty": ty_var(pin.ty),
                "doc": Var::from(pin.doc.as_str()),
            })
        })
        .collect()
}

fn ty_var(ty: Option<Ty>) -> Var {
    match ty {
        Some(ty) => Var::from(ty.to_string()),
        None => Var::Null, // the untyped marker
    }
}
