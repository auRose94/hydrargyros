//! The graph vocabulary and the conversion checker — the ONE validation
//! pass both compilers share (M3's JS, M4's Rust). **Conversion IS
//! validation**: a graph is valid if and only if [`Checker`] passes it,
//! and every failure is a [`GraphError`] whose coordinates name the node
//! and pin to click. Cycle failures carry their own cycle as the `flow`
//! chain — the graph's stack trace; shape/type/param failures keep an
//! empty flow (nothing was executing).
//!
//! The checker validates *meaning against the vocabulary* beyond the
//! model's document integrity (M2): kinds resolve (a function node's
//! callable must be in the function table), wires land on pins that
//! exist on the resolved descriptors, connected pin types agree, entry
//! nodes are entry-capable, pure-node params are present and valid, and
//! the exec/data graphs are cycle-free where the genre demands it (an
//! exec cycle has no straight-line form until a loop node exists in the
//! vocabulary; a data cycle is the implicit-garbage footgun — a wired
//! value cannot depend on a later execution).
//!
//! Generators run only on a checked graph and TRUST it — emission is
//! mechanical, validation never re-happens, so the two compilers cannot
//! disagree about what is valid. The descriptors double as the
//! inspector's source ([`super::inspect`]): every pin carries a doc,
//! every kind a summary and a doc link that resolves to a real file.

use std::collections::{HashMap, HashSet};

use crate::dynamic::functions::{function_meta, FunctionMeta};
use crate::dynamic::graph::model::{Graph, GraphError, Node};
use crate::var::Ty;

/// One pin of a node descriptor: a name, a type (`None` = any —
/// `web/request`'s `body`, function nodes whose callable declared no
/// signature), and a one-line doc for editor hovers and the inspector.
#[derive(Debug, Clone)]
pub struct PinSpec {
    /// The pin's name (an ASCII identifier; the wire schema names pins).
    pub name: String,
    /// The pin's type — `None` is the untyped marker (the value system's
    /// `Null` doubles as "any" in route-callable conventions).
    pub ty: Option<Ty>,
    /// One line for editor hovers and the inspector.
    pub doc: String,
}

/// A node kind's descriptor — what conversion checks wires against and
/// what the inspector shows. Built-ins are fixed; a function node's
/// descriptor is derived from its callable's [`FunctionMeta`].
#[derive(Debug, Clone)]
pub struct NodeSpec {
    /// The vocabulary kind (`"web/request"`, or a callable like
    /// `"greeter.greet"`).
    pub kind: String,
    /// One line for editors, hovers, and the inspector.
    pub summary: String,
    /// The node's documentation — a repo-relative path that resolves to
    /// a real file (`None` for function nodes: their doc is their
    /// origin's summary, carried by the table).
    pub doc: Option<String>,
    /// May be an entry node (starts an execution path).
    pub entry: bool,
    /// Receives exec (has an `exec` in-pin).
    pub exec_in: bool,
    /// The exec-out pin names, in codegen order.
    pub exec_outs: Vec<String>,
    /// The data-in pins.
    pub data_ins: Vec<PinSpec>,
    /// The data-out pins.
    pub data_outs: Vec<PinSpec>,
    /// A pure node has no exec pins and inlines as an expression at its
    /// use sites.
    pub pure: bool,
}

/// The doc link every built-in kind carries — the graph tier's design
/// section. `Option<String>` in the spec; the test asserts these resolve
/// to real files.
const GRAPH_DOC: &str = "docs/dynamic-design.md";

/// The built-in vocabulary + function-table kinds. The vocabulary
/// becomes a registry when a second registrar appears (shader graphs
/// register a GPU-shaped one — see dynamic-design.md); today it is one
/// lookup with two sources, which is the registry in embryo.
#[must_use]
pub fn spec_for(kind: &str) -> Option<NodeSpec> {
    match kind {
        "web/request" => Some(NodeSpec {
            kind: kind.to_string(),
            summary: "the route's entry — the staged request".into(),
            doc: Some(GRAPH_DOC.into()),
            entry: true,
            exec_in: false,
            exec_outs: vec!["exec".into()],
            data_ins: vec![],
            data_outs: vec![
                PinSpec {
                    name: "params".into(),
                    ty: Some(Ty::Object),
                    doc: "route parameters (from :param patterns)".into(),
                },
                PinSpec {
                    name: "query".into(),
                    ty: Some(Ty::Object),
                    doc: "the query string as an object".into(),
                },
                PinSpec {
                    name: "headers".into(),
                    ty: Some(Ty::Object),
                    doc: "the request headers".into(),
                },
                PinSpec {
                    name: "body".into(),
                    ty: None,
                    doc: "the parsed body — any value (JSON shape)".into(),
                },
            ],
            pure: false,
        }),
        "web/respond" => Some(NodeSpec {
            kind: kind.to_string(),
            summary: "finish the path — the response value".into(),
            doc: Some(GRAPH_DOC.into()),
            entry: false,
            exec_in: true,
            exec_outs: vec![],
            data_ins: vec![PinSpec {
                name: "body".into(),
                ty: None,
                doc: "the response value — Str serves as text, other values as JSON".into(),
            }],
            data_outs: vec![],
            pure: false,
        }),
        "flow/branch" => Some(NodeSpec {
            kind: kind.to_string(),
            summary: "choose then/else by a condition".into(),
            doc: Some(GRAPH_DOC.into()),
            entry: false,
            exec_in: true,
            exec_outs: vec!["then".into(), "else".into()],
            data_ins: vec![PinSpec {
                name: "condition".into(),
                ty: Some(Ty::Bool),
                doc: "falsy takes the else branch".into(),
            }],
            data_outs: vec![],
            pure: false,
        }),
        // pure nodes: expression-shaped, inlined at use sites
        "data/get" => Some(NodeSpec {
            kind: kind.to_string(),
            summary: "read one field of a value".into(),
            doc: Some(GRAPH_DOC.into()),
            entry: false,
            exec_in: false,
            exec_outs: vec![],
            data_ins: vec![PinSpec {
                name: "value".into(),
                ty: None,
                doc: "the value to read from (any shape)".into(),
            }],
            data_outs: vec![PinSpec {
                name: "out".into(),
                ty: None,
                doc: "the field's value (Null when absent)".into(),
            }],
            pure: true,
        }),
        "math/compare" => Some(NodeSpec {
            kind: kind.to_string(),
            summary: "compare two values (numeric or lexical)".into(),
            doc: Some(GRAPH_DOC.into()),
            entry: false,
            exec_in: false,
            exec_outs: vec![],
            data_ins: vec![
                PinSpec {
                    name: "a".into(),
                    ty: None,
                    doc: "either side — numbers compare numerically, strings lexically".into(),
                },
                PinSpec {
                    name: "b".into(),
                    ty: None,
                    doc: "either side — numbers compare numerically, strings lexically".into(),
                },
            ],
            data_outs: vec![PinSpec {
                name: "result".into(),
                ty: Some(Ty::Bool),
                doc: "true when the comparison holds".into(),
            }],
            pure: true,
        }),
        "data/literal" => Some(NodeSpec {
            kind: kind.to_string(),
            summary: "a constant value".into(),
            doc: Some(GRAPH_DOC.into()),
            entry: false,
            exec_in: false,
            exec_outs: vec![],
            data_ins: vec![],
            data_outs: vec![PinSpec {
                name: "out".into(),
                ty: None,
                doc: "the literal (its `value` param, checked at conversion)".into(),
            }],
            pure: true,
        }),
        _ => {
            // a callable's name IS a node kind — the function table's face
            let meta = function_meta(kind)?;
            Some(function_spec(kind, &meta))
        }
    }
}

/// A function node's descriptor IS its callable's signature — the third
/// face of the one function table (plugins registered in M1, scripts
/// bind, graph nodes call). Data pins are positional: `in0`, `in1`, …
/// in, `out` out. The doc is the origin's summary; the link is None
/// (the callable's home carries the docs).
pub(crate) fn function_spec(kind: &str, meta: &FunctionMeta) -> NodeSpec {
    NodeSpec {
        kind: kind.to_string(),
        summary: if meta.summary.is_empty() {
            format!("callable {:?} (origin: {})", kind, meta.origin)
        } else {
            meta.summary.clone()
        },
        doc: None,
        entry: false,
        exec_in: true,
        exec_outs: vec!["exec".into()],
        data_ins: meta
            .inputs
            .iter()
            .enumerate()
            .map(|(i, ty)| PinSpec {
                name: format!("in{i}"),
                ty: Some(*ty),
                doc: format!("positional argument {i} of {kind:?}"),
            })
            .collect(),
        data_outs: vec![PinSpec {
            name: "out".into(),
            ty: Some(meta.output),
            doc: "the callable's result".to_string(),
        }],
        pure: false,
    }
}

// ---- the conversion checker ----

pub(crate) struct Checker<'g> {
    /// The checked graph (generators read it freely).
    pub(crate) graph: &'g Graph,
    /// Resolved descriptors per node id.
    pub(crate) specs: HashMap<&'g str, NodeSpec>,
    /// Exec-wire adjacency: node id → [(from_pin, target id, target pin)].
    pub(crate) exec: HashMap<&'g str, Vec<(&'g str, &'g str, &'g str)>>,
    /// Exec paths from the entries (the flow chains): node → chain;
    /// first entry to reach a node owns its chain.
    pub(crate) paths: HashMap<&'g str, Vec<&'g str>>,
}

impl<'g> Checker<'g> {
    pub(crate) fn new(graph: &'g Graph) -> Result<Checker<'g>, GraphError> {
        // resolve every kind — an unknown kind fails with the node to click
        let mut specs = HashMap::new();
        for node in &graph.nodes {
            let spec = spec_for(&node.kind).ok_or_else(|| {
                let known = [
                    "web/request",
                    "web/respond",
                    "flow/branch",
                    "data/get",
                    "data/literal",
                    "math/compare",
                ];
                GraphError::at(
                    node.id.clone(),
                    None,
                    format!("unknown node kind {:?} (built-ins: {known:?}; a callable name must be registered in the function table)", node.kind),
                )
            })?;
            if specs.insert(node.id.as_str(), spec).is_some() {
                // M2 guarantees unique ids; this is belt-and-suspenders
                return Err(GraphError::at(node.id.clone(), None, "duplicate node id"));
            }
        }
        Ok(Checker {
            graph,
            specs,
            exec: HashMap::new(),
            paths: HashMap::new(),
        })
    }

    pub(crate) fn check(&mut self) -> Result<(), GraphError> {
        self.check_entries()?;
        self.check_wires()?;
        self.check_params()?;
        self.walk_exec_paths();
        self.check_exec_cycles()?;
        self.check_data_cycles()?;
        Ok(())
    }

    fn check_entries(&self) -> Result<(), GraphError> {
        for id in &self.graph.entry {
            let spec = &self.specs[id.as_str()];
            if !spec.entry {
                return Err(GraphError::at(
                    id.clone(),
                    None,
                    format!("{:?} cannot start an execution path (entry-capable kinds: web/request and future entry nodes)", spec.kind),
                ));
            }
        }
        Ok(())
    }

    fn check_wires(&mut self) -> Result<(), GraphError> {
        // index exec wires for the path walk and cycle checks
        let mut exec: HashMap<&str, Vec<(&str, &str, &str)>> = HashMap::new();

        for (i, wire) in self.graph.wires.iter().enumerate() {
            let from_node = self.graph.node(&wire.from).expect("M2 validated endpoints");
            let to_node = self.graph.node(&wire.to).expect("M2 validated endpoints");

            // pin existence, each side named for the click
            let from_side = if self.specs[&*from_node.id]
                .exec_outs
                .contains(&wire.from_pin)
            {
                "exec_out"
            } else if self.specs[&*from_node.id]
                .data_outs
                .iter()
                .any(|p| p.name == wire.from_pin)
            {
                "data_out"
            } else {
                let outs: Vec<&str> = self.specs[&*from_node.id]
                    .data_outs
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect();
                return Err(GraphError::at(
                    from_node.id.clone(),
                    wire.from_pin.clone(),
                    format!("wires[{i}]: {wire:?} — {:?} has no pin {:?} to wire from (exec outs: {:?}, data outs: {:?})",
                        from_node.kind, wire.from_pin,
                        self.specs[&*from_node.id].exec_outs, outs),
                ));
            };
            let to_side = if self.specs[&*to_node.id].exec_in && wire.to_pin == "exec" {
                "exec_in"
            } else if self.specs[&*to_node.id]
                .data_ins
                .iter()
                .any(|p| p.name == wire.to_pin)
            {
                "data_in"
            } else {
                let ins: Vec<&str> = self.specs[&*to_node.id]
                    .data_ins
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect();
                return Err(GraphError::at(
                    to_node.id.clone(),
                    wire.to_pin.clone(),
                    format!("wires[{i}]: {wire:?} — {:?} has no in-pin {:?} (exec in: {}, data ins: {:?})",
                        to_node.kind, wire.to_pin, self.specs[&*to_node.id].exec_in, ins),
                ));
            };

            // exec and data never mix — a pure node has neither
            let from_exec = from_side == "exec_out";
            let to_exec = to_side == "exec_in";
            if from_exec != to_exec {
                return Err(GraphError::at(
                    from_node.id.clone(),
                    wire.from_pin.clone(),
                    format!("wires[{i}]: {wire:?} — exec and data wires do not mix ({from_side} → {to_side})"),
                ));
            }

            // connected pins' types must agree (None = any)
            let ty_of = |node: &Node, pin: &str, side: &str| -> Option<Ty> {
                let spec = &self.specs[&*node.id];
                match side {
                    "exec_out" | "exec_in" => None,
                    "data_out" => spec
                        .data_outs
                        .iter()
                        .find(|p| p.name == pin)
                        .and_then(|p| p.ty),
                    _ => spec
                        .data_ins
                        .iter()
                        .find(|p| p.name == pin)
                        .and_then(|p| p.ty),
                }
            };
            let (src_ty, dst_ty) = (
                ty_of(from_node, &wire.from_pin.clone(), from_side),
                ty_of(to_node, &wire.to_pin.clone(), to_side),
            );
            if let (Some(src), Some(dst)) = (src_ty, dst_ty) {
                if src != dst {
                    return Err(GraphError::at(
                        to_node.id.clone(),
                        wire.to_pin.clone(),
                        format!(
                            "wires[{i}]: {wire:?} — pin type mismatch: {} outputs {}, {} expects {}",
                            from_node.id, src, to_node.id, dst,
                        ),
                    ));
                }
            }

            if from_exec {
                exec.entry(from_node.id.as_str()).or_default().push((
                    wire.from_pin.as_str(),
                    to_node.id.as_str(),
                    wire.to_pin.as_str(),
                ));
            }
        }
        self.exec = exec;
        Ok(())
    }

    /// BFS from the entries over exec wires — every reachable node gets
    /// the chain that reaches it (first entry wins). This IS the flow
    /// chain map: conversion failures walk it, and generated code exists
    /// only for reachable nodes (unreachable exec is editor-visible dead
    /// code that generates nothing).
    fn walk_exec_paths(&mut self) {
        let mut queue: std::collections::VecDeque<&str> = std::collections::VecDeque::new();
        for id in &self.graph.entry {
            self.paths
                .entry(id.as_str())
                .or_insert_with(|| vec![id.as_str()]);
            queue.push_back(id.as_str());
        }
        while let Some(id) = queue.pop_front() {
            let chain = self.paths[id].clone();
            if let Some(outs) = self.exec.get(id) {
                for &(_, target, _) in outs {
                    if !self.paths.contains_key(target) {
                        let mut path = chain.clone();
                        path.push(target);
                        self.paths.insert(target, path);
                        queue.push_back(target);
                    }
                }
            }
        }
    }

    /// Pure-node params are vocabulary semantics, so they are checked
    /// HERE, at conversion — generators then trust them and only carry
    /// bug-guards. (Before this pass moved into the checker, M3's
    /// generator validated its own params; one checker means both
    /// compilers get the same refusal for the same graph.)
    fn check_params(&self) -> Result<(), GraphError> {
        for node in &self.graph.nodes {
            match self.specs[node.id.as_str()].kind.as_str() {
                "data/get" => {
                    if node.params.get("key").as_str().is_none() {
                        return Err(GraphError::at(
                            node.id.clone(),
                            None,
                            "\"key\" param is required — the field to read (a string)",
                        ));
                    }
                }
                "math/compare" => {
                    let op_var = node.params.get("op");
                    let Some(op) = op_var.as_str() else {
                        return Err(GraphError::at(
                            node.id.clone(),
                            None,
                            "\"op\" param is required — one of < > == != <= >=",
                        ));
                    };
                    if !["<", ">", "==", "!=", "<=", ">="].contains(&op) {
                        return Err(GraphError::at(
                            node.id.clone(),
                            None,
                            format!("\"op\" param {op:?} is not a comparison (< > == != <= >=)"),
                        ));
                    }
                }
                "data/literal" => {
                    if matches!(node.params.get("value"), crate::var::Var::Null) {
                        return Err(GraphError::at(
                            node.id.clone(),
                            None,
                            "\"value\" param is required — the literal to emit (a JSON-shaped Var)",
                        ));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Exec cycles fail conversion — until a loop node exists in the
    /// vocabulary, an exec cycle has no generated form. The error's flow
    /// chain IS the cycle, walked from the entry.
    fn check_exec_cycles(&self) -> Result<(), GraphError> {
        let mut done: HashSet<&str> = HashSet::new();
        for id in &self.graph.entry {
            let mut stack: Vec<&str> = vec![id.as_str()];
            let mut on_stack: HashSet<&str> = HashSet::from([id.as_str()]);
            if let Some(err) = self.exec_dfs(id.as_str(), &mut stack, &mut on_stack, &mut done) {
                return Err(err);
            }
        }
        Ok(())
    }

    fn exec_dfs(
        &self,
        id: &'g str,
        stack: &mut Vec<&'g str>,
        on_stack: &mut HashSet<&'g str>,
        done: &mut HashSet<&'g str>,
    ) -> Option<GraphError> {
        for &(_, target, _) in self.exec.get(id).into_iter().flatten() {
            if on_stack.contains(target) {
                // the cycle, walked backwards from the revisit to the entry
                let pos = stack.iter().position(|&n| n == target).expect("on stack");
                let flow: Vec<String> = stack[pos..]
                    .iter()
                    .map(|n| (*n).to_string())
                    .chain(std::iter::once((*target).to_string()))
                    .collect();
                return Some(GraphError {
                    node: Some((*target).to_string()),
                    pin: None,
                    message: format!(
                        "exec cycle {} — a loop needs a loop node in the vocabulary; \
                         straight-line code has no generated form for this",
                        flow.join(" → ")
                    ),
                    flow,
                });
            }
            if done.contains(target) {
                continue;
            }
            done.insert(target);
            stack.push(target);
            on_stack.insert(target);
            if let Some(err) = self.exec_dfs(target, stack, on_stack, done) {
                return Some(err);
            }
            stack.pop();
            on_stack.remove(target);
        }
        None
    }

    /// Data cycles fail conversion — a value defined by a later execution
    /// is the implicit-garbage footgun by another name. The data graph:
    /// every data wire's source node → target node.
    fn check_data_cycles(&self) -> Result<(), GraphError> {
        let mut data: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
        for wire in &self.graph.wires {
            let from_spec = &self.specs[&*self.graph.node(&wire.from).expect("M2").id];
            let to_spec = &self.specs[&*self.graph.node(&wire.to).expect("M2").id];
            let from_is_data = from_spec.data_outs.iter().any(|p| p.name == wire.from_pin);
            let to_is_data = to_spec.data_ins.iter().any(|p| p.name == wire.to_pin);
            if from_is_data && to_is_data {
                data.entry(wire.from.as_str())
                    .or_default()
                    .push((wire.from_pin.as_str(), wire.to.as_str()));
            }
        }
        let mut done: HashSet<&str> = HashSet::new();
        for node in &self.graph.nodes {
            let mut stack: Vec<&str> = vec![node.id.as_str()];
            let mut on_stack: HashSet<&str> = HashSet::from([node.id.as_str()]);
            if let Some(err) = self.data_dfs(
                node.id.as_str(),
                &data,
                &mut stack,
                &mut on_stack,
                &mut done,
            ) {
                return Err(err);
            }
        }
        Ok(())
    }

    fn data_dfs(
        &self,
        id: &'g str,
        data: &HashMap<&'g str, Vec<(&'g str, &'g str)>>,
        stack: &mut Vec<&'g str>,
        on_stack: &mut HashSet<&'g str>,
        done: &mut HashSet<&'g str>,
    ) -> Option<GraphError> {
        for &(_, target) in data.get(id).into_iter().flatten() {
            if on_stack.contains(target) {
                let pos = stack.iter().position(|&n| n == target).expect("on stack");
                let flow: Vec<String> = stack[pos..]
                    .iter()
                    .map(|n| (*n).to_string())
                    .chain(std::iter::once((*target).to_string()))
                    .collect();
                return Some(GraphError {
                    node: Some((*target).to_string()),
                    pin: None,
                    message: format!(
                        "data cycle {} — a wired value cannot depend on a later execution",
                        flow.join(" → ")
                    ),
                    flow,
                });
            }
            if done.contains(target) {
                continue;
            }
            done.insert(target);
            stack.push(target);
            on_stack.insert(target);
            if let Some(err) = self.data_dfs(target, data, stack, on_stack, done) {
                return Some(err);
            }
            stack.pop();
            on_stack.remove(target);
        }
        None
    }
}
