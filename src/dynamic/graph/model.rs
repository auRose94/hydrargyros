//! The graph document model — the Var schema for Blueprint-style visual
//! programs and its structural validation. Pure data, no feature gate.
//!
//! # The document
//!
//! A graph is a serialized `Var` riding the `.hyg` carrier convention
//! (`graph.json.hyg` while authoring — diffable; `.cbor.hyg` when
//! machines exchange big graphs):
//!
//! ```json
//! {
//!   "graph": "order-flow",
//!   "format": 1,
//!   "nodes": [
//!     { "id": "req1", "type": "web/request", "pos": [80, 120] },
//!     { "id": "branch1", "type": "flow/branch", "pos": [320, 140],
//!       "params": { "threshold": 100 } },
//!     { "id": "resp1", "type": "web/respond", "pos": [560, 110] }
//!   ],
//!   "wires": [
//!     { "from": "req1", "from_pin": "exec", "to": "branch1", "to_pin": "exec_in" },
//!     { "from": "branch1", "from_pin": "then", "to": "resp1", "to_pin": "exec_in" }
//!   ],
//!   "entry": ["req1"]
//! }
//! ```
//!
//! (The design sketch joined wire endpoints as `"pin"` on both ends —
//! duplicate keys are unrepresentable in JSON, so the landed schema names
//! each side: `from_pin`/`to_pin`.)
//!
//! # What M2 validates (structural, vocabulary-blind BY DESIGN)
//!
//! The model knows document integrity, not node semantics: unique
//! identifier node ids, wires that land on existing nodes, pins that are
//! named, entry lists that reference real nodes, and the format gate.
//! It deliberately does NOT know what pins a `flow/branch` has or which
//! are exec vs data — that is the node vocabulary's job, registered by
//! the web/game modules and plugins (a plugin's manifest functions make
//! them graph nodes automatically), and **type checking is pin checking**
//! happens at conversion — a graph is valid if and only if it converts
//! (the compilers' contract, next milestones). Consequences, on purpose:
//!
//! - **Fan-out is legal** — one exec pin feeding several targets.
//! - **Cycles are legal** — loops are real; flow nodes close them.
//! - **Orphans are legal** — entry nodes and data sources may wire to
//!   nothing; sub-graphs (an empty `entry`) are called, not launched.
//!
//! # Errors carry graph coordinates
//!
//! Every validation failure is a [`GraphError`] naming the **node** and
//! the **pin** to click — the same voice the compilers will use when
//! conversion failures walk the flow chain (the graph's stack trace).
//! Structural errors have an empty `flow` (nothing was executing).

use std::fmt;

use crate::dynamic::engine::validate_identifier;
use crate::list::List;
use crate::o;
use crate::var::Var;

/// The document schema version this model reads and writes. A document
/// with a different `format` is an explicit error — per-node migrations
/// (`Var → Var` functions) are the versioning path, not silent best-effort.
pub const GRAPH_FORMAT: i64 = 1;

/// A graph-validation failure with graph coordinates. `node`/`pin` name
/// what to click in an editor; `flow` is the walked chain for conversion
/// failures (empty for structural errors — nothing was executing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphError {
    /// The node to click (the one that failed or hosts the failing pin).
    pub node: Option<String>,
    /// The pin to look at, when the failure is pin-specific.
    pub pin: Option<String>,
    /// What failed and why.
    pub message: String,
    /// The flow chain — walked backwards from the failure to the entry
    /// node by the compilers (M3+); empty for structural errors.
    pub flow: Vec<String>,
}

impl GraphError {
    /// A structural failure naming the node (and pin, when known) that
    /// caused it.
    pub fn at(
        node: impl Into<Option<String>>,
        pin: impl Into<Option<String>>,
        message: impl Into<String>,
    ) -> GraphError {
        GraphError {
            node: node.into(),
            pin: pin.into(),
            message: message.into(),
            flow: Vec::new(),
        }
    }
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.node, &self.pin) {
            (Some(node), Some(pin)) => write!(f, "{node}.{pin}: {}", self.message),
            (Some(node), None) => write!(f, "{node}: {}", self.message),
            _ => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for GraphError {}

/// A parsed graph document — the authoritative in-memory form.
#[derive(Debug, Clone, PartialEq)]
pub struct Graph {
    /// The graph's name (`"graph"` — a document name, not a namespace).
    pub name: String,
    /// The nodes, in document order.
    pub nodes: Vec<Node>,
    /// The wires, in document order.
    pub wires: Vec<Wire>,
    /// The entry nodes — where execution starts. Empty for sub-graphs
    /// (called by function nodes, never launched directly).
    pub entry: Vec<String>,
}

/// One node in the graph.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// The node's id — unique within the graph, an ASCII identifier
    /// (errors, paths, and generated code all name nodes by it).
    pub id: String,
    /// The node's vocabulary kind (`"type"` in the document —
    /// `"web/request"`, `"flow/branch"`, or a callable like
    /// `"greeter.greet"`). Not interpreted by this layer.
    pub kind: String,
    /// The editor position, kept RAW (it is editor state, not engine
    /// state, and preserving the numeric spelling keeps round-trips
    /// byte-identical — JSON's `80` and `80.0` are different bytes).
    pub pos: Option<Var>,
    /// The node's parameters — a free-form `Var` object (pin defaults,
    /// thresholds, paths). Not interpreted by this layer.
    pub params: Var,
}

/// One wire: a pin on one node to a pin on another.
#[derive(Debug, Clone, PartialEq)]
pub struct Wire {
    /// The source node's id.
    pub from: String,
    /// The source pin's name.
    pub from_pin: String,
    /// The target node's id.
    pub to: String,
    /// The target pin's name.
    pub to_pin: String,
}

impl Graph {
    /// Parse and structurally validate a graph document.
    ///
    /// # Errors
    /// A [`GraphError`] naming the first structural failure — wrong
    /// shape, unknown format, duplicate or non-identifier node ids,
    /// wires/entries landing on unknown nodes, unnamed pins, or an exact
    /// duplicate wire. Unknown *extra* fields are tolerated (forward
    /// compatibility); missing `nodes`/`wires`/`entry` are empty (a bare
    /// `{ "graph", "format" }` is a valid empty graph).
    pub fn from_var(v: &Var) -> Result<Graph, GraphError> {
        let doc = v.as_object().ok_or_else(|| {
            GraphError::at(
                None,
                None,
                format!("graph document: expected an object, found {}", v.ty()),
            )
        })?;

        // the name — a non-empty string
        let name = doc
            .get("graph")
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                GraphError::at(
                    None,
                    None,
                    "graph document: \"graph\" (the name) is required — a non-empty string"
                        .to_string(),
                )
            })?
            .to_string();

        // the format gate — the versioning hook
        let format = doc.get("format").as_i64().ok_or_else(|| {
            GraphError::at(
                None,
                None,
                "graph document: \"format\" is required — an integer".to_string(),
            )
        })?;
        if format != GRAPH_FORMAT {
            return Err(GraphError::at(
                None,
                None,
                format!("\"format\": {format} — this hyg speaks graph format {GRAPH_FORMAT}"),
            ));
        }

        // nodes — missing is empty
        let mut nodes = Vec::new();
        let nodes_var = doc.get("nodes");
        if !matches!(nodes_var, Var::Null) {
            let nodes_list = nodes_var.as_list().ok_or_else(|| {
                GraphError::at(
                    None,
                    None,
                    format!("\"nodes\": expected a list, found {}", nodes_var.ty()),
                )
            })?;
            for (i, item) in nodes_list.iter().iter().enumerate() {
                let node = parse_node(item).map_err(|mut e| {
                    e.message = format!("nodes[{i}]: {}", e.message);
                    e
                })?;
                if nodes.iter().any(|n: &Node| n.id == node.id) {
                    return Err(GraphError::at(
                        node.id.clone(),
                        None,
                        format!(
                            "duplicate node id {:?} (ids are unique within a graph)",
                            node.id
                        ),
                    ));
                }
                nodes.push(node);
            }
        }

        // wires — missing is empty; endpoints and pins are checked here
        let mut wires = Vec::new();
        let wires_var = doc.get("wires");
        if !matches!(wires_var, Var::Null) {
            let wires_list = wires_var.as_list().ok_or_else(|| {
                GraphError::at(
                    None,
                    None,
                    format!("\"wires\": expected a list, found {}", wires_var.ty()),
                )
            })?;
            for (i, item) in wires_list.iter().iter().enumerate() {
                let wire = parse_wire(item).map_err(|mut e| {
                    e.message = format!("wires[{i}]: {}", e.message);
                    e
                })?;
                for (side, endpoint) in [("from", &wire.from), ("to", &wire.to)] {
                    if !nodes.iter().any(|n| n.id == *endpoint) {
                        let known: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
                        return Err(GraphError::at(
                            endpoint.clone(),
                            None,
                            format!(
                                "wire {side} lands on unknown node {endpoint:?} (known: {known:?})"
                            ),
                        ));
                    }
                }
                if wires.iter().any(|w| w == &wire) {
                    return Err(GraphError::at(
                        wire.from.clone(),
                        wire.from_pin.clone(),
                        format!("duplicate wire {wire:?} (identical wires are noise; fan-out is legal — this exact wire exists already)"),
                    ));
                }
                wires.push(wire);
            }
        }

        // entry — missing is empty (sub-graphs are called, not launched)
        let mut entry = Vec::new();
        let entry_var = doc.get("entry");
        if !matches!(entry_var, Var::Null) {
            let entry_list = entry_var.as_list().ok_or_else(|| {
                GraphError::at(
                    None,
                    None,
                    format!("\"entry\": expected a list, found {}", entry_var.ty()),
                )
            })?;
            for (i, item) in entry_list.iter().iter().enumerate() {
                let id = item.as_str().ok_or_else(|| {
                    GraphError::at(
                        None,
                        None,
                        format!("entry[{i}]: expected a node-id string, found {}", item.ty()),
                    )
                })?;
                if !nodes.iter().any(|n| n.id == id) {
                    let known: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
                    return Err(GraphError::at(
                        id.to_string(),
                        None,
                        format!("entry[{i}] names unknown node {id:?} (known: {known:?})"),
                    ));
                }
                entry.push(id.to_string());
            }
        }

        Ok(Graph {
            name,
            nodes,
            wires,
            entry,
        })
    }

    /// The canonical document `Var` — sketch field order (`graph`,
    /// `format`, `nodes`, `wires`, `entry`), nodes in document order.
    /// This regenerates the schema's fields, not unknown extras: the
    /// carrier's round-trip identity is through the `Var` itself; this is
    /// the build-artifact regeneration path.
    #[must_use]
    pub fn to_var(&self) -> Var {
        let nodes: List = self.nodes.iter().map(Node::to_var).collect();
        let wires: List = self.wires.iter().map(Wire::to_var).collect();
        let entry: List = self.entry.iter().map(|id| Var::from(id.as_str())).collect();
        Var::from(o! {
            "graph": Var::from(self.name.as_str()),
            "format": Var::from(GRAPH_FORMAT),
            "nodes": Var::from(nodes),
            "wires": Var::from(wires),
            "entry": Var::from(entry),
        })
    }

    /// The node with this id, if the graph has one.
    #[must_use]
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

impl Node {
    /// The editor position as `(x, y)` — for consumers that want numbers
    /// rather than the raw `Var` ([`pos`](Node::pos) stays raw).
    #[must_use]
    pub fn pos_xy(&self) -> Option<(f64, f64)> {
        let list = self.pos.as_ref()?.as_list()?;
        let items = list.iter();
        let x = items.first()?.as_f64()?;
        let y = items.get(1)?.as_f64()?;
        Some((x, y))
    }

    /// The canonical node `Var` (`id`, `type`, then editor state).
    #[must_use]
    pub fn to_var(&self) -> Var {
        let node = o! {
            "id": Var::from(self.id.as_str()),
            "type": Var::from(self.kind.as_str()),
        };
        if let Some(pos) = &self.pos {
            node.set("pos", pos.clone());
        }
        if !matches!(self.params, Var::Null) {
            node.set("params", self.params.clone());
        }
        Var::from(node)
    }
}

impl Wire {
    /// The canonical wire `Var`.
    #[must_use]
    pub fn to_var(&self) -> Var {
        Var::from(o! {
            "from": Var::from(self.from.as_str()),
            "from_pin": Var::from(self.from_pin.as_str()),
            "to": Var::from(self.to.as_str()),
            "to_pin": Var::from(self.to_pin.as_str()),
        })
    }
}

fn parse_node(v: &Var) -> Result<Node, GraphError> {
    let obj = v.as_object().ok_or_else(|| {
        GraphError::at(None, None, format!("expected an object, found {}", v.ty()))
    })?;

    let id = obj
        .get("id")
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            GraphError::at(
                None,
                None,
                "\"id\" is required — a non-empty string".to_string(),
            )
        })?
        .to_string();
    // the node id names paths, errors, and generated code — identifier only
    validate_identifier(&id)
        .map_err(|e| GraphError::at(id.clone(), None, format!("\"id\": {e}")))?;

    let kind = obj
        .get("type")
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            GraphError::at(
                id.clone(),
                None,
                "\"type\" is required — the node's vocabulary kind".to_string(),
            )
        })?
        .to_string();

    // pos: editor state — kept raw; validated as a 2-number list when present
    let pos_var = obj.get("pos");
    let pos = if matches!(pos_var, Var::Null) {
        None
    } else {
        let list = pos_var.as_list().ok_or_else(|| {
            GraphError::at(
                id.clone(),
                None,
                format!("\"pos\": expected a 2-number list, found {}", pos_var.ty()),
            )
        })?;
        if list.len() != 2 {
            return Err(GraphError::at(
                id.clone(),
                None,
                format!("\"pos\": expected a 2-number list, got {}", list.len()),
            ));
        }
        for item in list.iter().iter() {
            if item.as_f64().is_none() {
                return Err(GraphError::at(
                    id.clone(),
                    None,
                    format!("\"pos\": expected numbers, found {}", item.ty()),
                ));
            }
        }
        Some(pos_var.clone())
    };

    // params: engine state — a free-form object, kept raw
    let params = obj.get("params");
    if !matches!(params, Var::Null) && params.as_object().is_none() {
        return Err(GraphError::at(
            id.clone(),
            None,
            format!("\"params\": expected an object, found {}", params.ty()),
        ));
    }

    Ok(Node {
        id,
        kind,
        pos,
        params,
    })
}

fn parse_wire(v: &Var) -> Result<Wire, GraphError> {
    let obj = v.as_object().ok_or_else(|| {
        GraphError::at(None, None, format!("expected an object, found {}", v.ty()))
    })?;

    let from = obj
        .get("from")
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            GraphError::at(
                None,
                None,
                "\"from\" is required — the source node's id".to_string(),
            )
        })?
        .to_string();
    let from_pin = obj
        .get("from_pin")
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            GraphError::at(
                from.clone(),
                None,
                "\"from_pin\" is required — the source pin's name".to_string(),
            )
        })?
        .to_string();
    // pins are named endpoints — the vocabulary decides their types, the
    // document still has to name them validly
    validate_identifier(&from_pin).map_err(|e| {
        GraphError::at(from.clone(), from_pin.clone(), format!("\"from_pin\": {e}"))
    })?;

    let to = obj
        .get("to")
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            GraphError::at(
                from.clone(),
                from_pin.clone(),
                "\"to\" is required — the target node's id".to_string(),
            )
        })?
        .to_string();
    let to_pin = obj
        .get("to_pin")
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            GraphError::at(
                from.clone(),
                from_pin.clone(),
                "\"to_pin\" is required — the target pin's name".to_string(),
            )
        })?
        .to_string();
    validate_identifier(&to_pin)
        .map_err(|e| GraphError::at(from.clone(), from_pin.clone(), format!("\"to_pin\": {e}")))?;

    Ok(Wire {
        from,
        from_pin,
        to,
        to_pin,
    })
}
