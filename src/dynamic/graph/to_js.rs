//! The graph → JavaScript compiler — the script seam's hot path and the
//! first place **conversion IS validation** gets teeth: a graph is valid
//! if and only if this compiles it, and every failure is a
//! [`GraphError`] whose `flow` chain is *walked* — the exec path from the
//! entry node backwards to the failure, the graph's stack trace.
//!
//! # The generated shape
//!
//! One graph compiles to one JS function, `__hyg_handle(req)`: the staged
//! request `Var` in, the response value out (a `web/respond` node is a
//! `return`; a path that never reaches one returns `undefined`, which the
//! bridge reads as [`Var::Null`]). Event-graph codegen is straight-line:
//!
//! - non-pure nodes pre-declare their data-out pins as function-scope
//!   `let`s, so a value produced inside a branch flows out of it (the
//!   Blueprint convergence shape);
//! - **pure nodes** (expression nodes — `data/get`, `data/literal`, `math/compare`) have
//!   no exec pins and inline at their use sites as expressions, so a
//!   pure-node cycle is an *expression* cycle and fails conversion
//!   naming the chain;
//! - **function nodes** (kind = a callable name — `greeter.greet`) are
//!   the function table's third face: their pins ARE the callable's
//!   [`FunctionMeta`](crate::dynamic::functions::FunctionMeta) pins, and
//!   execution dispatches through the host binding `__hyg_node_call`,
//!   which lands in
//!   [`call_function`](crate::dynamic::functions::call_function) — the
//!   same table plugins registered into (M1) and scripts bind into;
//! - exec fan-out generates sequential walks (both targets run, in wire
//!   order); converging paths re-execute the shared tail per arrival,
//!   the Blueprint semantics.
//!
//! # What conversion validates (beyond the model's structure)
//!
//! The model (M2) validated document integrity; the compiler validates
//! *meaning* against the vocabulary: kinds resolve (a function node's
//! callable must be in the table), wires land on pins that exist on the
//! resolved descriptors, connected pin types agree, entry nodes are
//! entry-capable, pure expressions terminate, and the exec graph is a
//! DAG — loops are real, but until a loop node exists in the vocabulary
//! an exec cycle has no generated form, so it fails conversion carrying
//! its own cycle as the `flow` chain. Data cycles (a wire cycle between
//! nodes) fail the same way — reading a value that is defined by a later
//! execution is the implicit-garbage footgun by another name.
//!
//! Pure data, no feature gate — the JS source compiles and validates
//! without an engine; running it is the `script` feature's job (see
//! [`super::route`]).

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use crate::dynamic::functions::{function_meta, FunctionMeta};
use crate::dynamic::graph::model::{Graph, GraphError, Node};
use crate::var::{Ty, Var};

/// The generated route's entry function — what the host calls with the
/// staged request.
pub const ROUTE_FN: &str = "__hyg_handle";
/// The host binding function nodes dispatch through (bound by
/// [`super::route`] with a closure into the function table).
pub const NODE_CALL_HOST: &str = "__hyg_node_call";

/// A node kind's descriptor — what conversion checks wires against.
/// Built-ins are fixed; a function node's descriptor is derived from its
/// callable's [`FunctionMeta`].
#[derive(Debug, Clone)]
pub struct NodeSpec {
    /// The vocabulary kind (`"web/request"`, or a callable like
    /// `"greeter.greet"`).
    pub kind: String,
    /// May be an entry node (starts an execution path).
    pub entry: bool,
    /// Receives exec (has an `exec` in-pin).
    pub exec_in: bool,
    /// The exec-out pin names, in codegen order.
    pub exec_outs: Vec<String>,
    /// The data-in pins.
    pub data_ins: Vec<(String, Option<Ty>)>,
    /// The data-out pins.
    pub data_outs: Vec<(String, Option<Ty>)>,
    /// A pure node has no exec pins and inlines as an expression at its
    /// use sites.
    pub pure: bool,
}

/// The M3 built-in vocabulary + function-table kinds. The vocabulary
/// becomes a registry when a second registrar appears (shader graphs
/// register a GPU-shaped one — see dynamic-design.md); today it is one
/// lookup with two sources, which is the registry in embryo.
#[must_use]
pub fn spec_for(kind: &str) -> Option<NodeSpec> {
    match kind {
        "web/request" => Some(NodeSpec {
            kind: kind.to_string(),
            entry: true,
            exec_in: false,
            exec_outs: vec!["exec".into()],
            data_ins: vec![],
            data_outs: vec![
                ("params".into(), Some(Ty::Object)),
                ("query".into(), Some(Ty::Object)),
                ("headers".into(), Some(Ty::Object)),
                ("body".into(), None),
            ],
            pure: false,
        }),
        "web/respond" => Some(NodeSpec {
            kind: kind.to_string(),
            entry: false,
            exec_in: true,
            exec_outs: vec![],
            data_ins: vec![("body".into(), None)],
            data_outs: vec![],
            pure: false,
        }),
        "flow/branch" => Some(NodeSpec {
            kind: kind.to_string(),
            entry: false,
            exec_in: true,
            exec_outs: vec!["then".into(), "else".into()],
            data_ins: vec![("condition".into(), Some(Ty::Bool))],
            data_outs: vec![],
            pure: false,
        }),
        // pure nodes: expression-shaped, inlined at use sites
        "data/get" => Some(NodeSpec {
            kind: kind.to_string(),
            entry: false,
            exec_in: false,
            exec_outs: vec![],
            data_ins: vec![("value".into(), None)],
            data_outs: vec![("out".into(), None)],
            pure: true,
        }),
        "math/compare" => Some(NodeSpec {
            kind: kind.to_string(),
            entry: false,
            exec_in: false,
            exec_outs: vec![],
            data_ins: vec![("a".into(), None), ("b".into(), None)],
            data_outs: vec![("result".into(), Some(Ty::Bool))],
            pure: true,
        }),
        "data/literal" => Some(NodeSpec {
            kind: kind.to_string(),
            entry: false,
            exec_in: false,
            exec_outs: vec![],
            data_ins: vec![],
            data_outs: vec![("out".into(), None)],
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
/// in, `out` out.
fn function_spec(kind: &str, meta: &FunctionMeta) -> NodeSpec {
    NodeSpec {
        kind: kind.to_string(),
        entry: false,
        exec_in: true,
        exec_outs: vec!["exec".into()],
        data_ins: meta
            .inputs
            .iter()
            .enumerate()
            .map(|(i, ty)| (format!("in{i}"), Some(*ty)))
            .collect(),
        data_outs: vec![("out".into(), Some(meta.output))],
        pure: false,
    }
}

/// The compiled artifact: generated JS plus what the route layer needs
/// to materialize it. Feature-free — the source is data until a `script`
/// engine runs it.
#[derive(Debug, Clone)]
pub struct CompiledRoute {
    /// The generated JS (defines [`ROUTE_FN`]).
    pub js: String,
    /// The graph's name (route introspection/labels).
    pub graph_name: String,
}

impl CompiledRoute {
    /// The graph's name (route introspection/labels).
    #[must_use]
    pub fn graph_name(&self) -> &str {
        &self.graph_name
    }
}

/// Compile a graph document to JS — conversion IS validation.
///
/// # Errors
/// A [`GraphError`] naming the first failure: an unknown kind (function
/// nodes must resolve in the callable table), a wire landing on a pin
/// the descriptor doesn't have, connected pins whose types disagree, an
/// entry node that isn't entry-capable, a pure-expression cycle, an
/// exec cycle, or a data cycle — the cycle errors carry the walked
/// chain, the graph's stack trace.
pub fn compile(graph: &Graph) -> Result<CompiledRoute, GraphError> {
    let mut checker = Checker::new(graph)?;
    checker.check()?;
    let js = generate(graph)?;
    Ok(CompiledRoute {
        js,
        graph_name: graph.name.clone(),
    })
}

// ---- the conversion checker ----

struct Checker<'g> {
    graph: &'g Graph,
    /// Resolved descriptors per node id.
    specs: HashMap<&'g str, NodeSpec>,
    /// Exec-wire adjacency: node id → [(from_pin, target id, target pin)].
    exec: HashMap<&'g str, Vec<(&'g str, &'g str, &'g str)>>,
    /// Exec paths from the entries (the flow chains): node → entry →
    /// chain. First entry to reach a node owns its chain.
    paths: HashMap<&'g str, Vec<&'g str>>,
}

impl<'g> Checker<'g> {
    fn new(graph: &'g Graph) -> Result<Checker<'g>, GraphError> {
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

    fn check(&mut self) -> Result<(), GraphError> {
        self.check_entries()?;
        self.check_wires()?;
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
                .any(|(p, _)| *p == wire.from_pin)
            {
                "data_out"
            } else {
                return Err(GraphError::at(
                    from_node.id.clone(),
                    wire.from_pin.clone(),
                    format!("wires[{i}]: {wire:?} — {:?} has no pin {:?} to wire from (exec outs: {:?}, data outs: {:?})",
                        from_node.kind, wire.from_pin,
                        self.specs[&*from_node.id].exec_outs, self.specs[&*from_node.id].data_outs.iter().map(|(p, _)| p).collect::<Vec<_>>()),
                ));
            };
            let to_side = if self.specs[&*to_node.id].exec_in && wire.to_pin == "exec" {
                "exec_in"
            } else if self.specs[&*to_node.id]
                .data_ins
                .iter()
                .any(|(p, _)| *p == wire.to_pin)
            {
                "data_in"
            } else {
                return Err(GraphError::at(
                    to_node.id.clone(),
                    wire.to_pin.clone(),
                    format!("wires[{i}]: {wire:?} — {:?} has no in-pin {:?} (exec in: {}, data ins: {:?})",
                        to_node.kind, wire.to_pin, self.specs[&*to_node.id].exec_in,
                        self.specs[&*to_node.id].data_ins.iter().map(|(p, _)| p).collect::<Vec<_>>()),
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
                        .find(|(p, _)| p == pin)
                        .and_then(|(_, t)| *t),
                    _ => spec
                        .data_ins
                        .iter()
                        .find(|(p, _)| p == pin)
                        .and_then(|(_, t)| *t),
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
            let from_is_data = from_spec.data_outs.iter().any(|(p, _)| *p == wire.from_pin);
            let to_is_data = to_spec.data_ins.iter().any(|(p, _)| *p == wire.to_pin);
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

// ---- the generator (runs only on a checked graph) ----

fn generate(graph: &Graph) -> Result<String, GraphError> {
    // rebuild the exec adjacency + paths for codegen (the checker owns
    // them; codegen needs the same walk)
    let mut exec: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
    for wire in &graph.wires {
        let from_spec = spec_for(&graph.node(&wire.from).expect("M2").kind).ok_or_else(|| {
            GraphError::at(
                wire.from.clone(),
                None,
                format!(
                    "kind {:?} stopped resolving in codegen (a checker bug)",
                    wire.from
                ),
            )
        })?;
        if from_spec.exec_outs.contains(&wire.from_pin) {
            exec.entry(wire.from.as_str())
                .or_default()
                .push((wire.from_pin.as_str(), wire.to.as_str()));
        }
    }
    let mut queue = std::collections::VecDeque::new();
    let mut paths: HashMap<&str, Vec<&str>> = HashMap::new();
    for id in &graph.entry {
        paths
            .entry(id.as_str())
            .or_insert_with(|| vec![id.as_str()]);
        queue.push_back(id.as_str());
    }
    while let Some(id) = queue.pop_front() {
        let chain = paths[id].clone();
        if let Some(outs) = exec.get(id) {
            for &(_, target) in outs {
                if !paths.contains_key(target) {
                    let mut path = chain.clone();
                    path.push(target);
                    paths.insert(target, path);
                    queue.push_back(target);
                }
            }
        }
    }

    // every reachable non-pure node's data-out pins pre-declared as
    // function-scope lets (values produced in a branch flow out of it)
    let mut lets = String::new();
    let mut body = String::new();
    let nodes_by_id: HashMap<&str, &Node> =
        graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    let mut declared: HashSet<String> = HashSet::new();
    for id in &graph.entry {
        emit_node(
            graph,
            &nodes_by_id,
            &exec,
            id.as_str(),
            &mut declared,
            &mut lets,
            &mut body,
            0,
        )?;
    }

    let mut js = format!("function {ROUTE_FN}(req) {{\n");
    js.push_str(&lets);
    js.push_str(&body);
    js.push_str("}\n");
    Ok(js)
}

/// JSON string literal (JSON string syntax is valid JS string syntax —
/// the bridge's own trick).
fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// The JS expression for a data input: the wired source's value — a let
/// reference for non-pure sources, an inlined expression for pure ones.
/// Unwired pins read `undefined` (the JS coercion rules apply — pin
/// defaults are an editor concern until a vocabulary carries them).
fn value_expr(
    graph: &Graph,
    nodes_by_id: &HashMap<&str, &Node>,
    target: &str,
    pin: &str,
    depth: usize,
) -> Result<String, GraphError> {
    let wire = graph
        .wires
        .iter()
        .find(|w| w.to == target && w.to_pin == pin);
    let Some(wire) = wire else {
        return Ok("undefined".to_string());
    };
    let src = nodes_by_id[wire.from.as_str()];
    let src_spec = spec_for(&src.kind).expect("checked");
    if src_spec.pure {
        // inline the pure node's expression (depth-bounded by the
        // conversion check: pure cycles failed already)
        if depth > 64 {
            return Err(GraphError::at(
                src.id.clone(),
                wire.from_pin.clone(),
                "pure-node expression nesting exceeded 64 (a cycle slipped past the check — a compiler bug)",
            ));
        }
        pure_expr(graph, nodes_by_id, src, depth + 1)
    } else {
        Ok(format!("{}_{}", src.id, wire.from_pin))
    }
}

/// The pure node's inline expression.
fn pure_expr(
    graph: &Graph,
    nodes_by_id: &HashMap<&str, &Node>,
    node: &Node,
    depth: usize,
) -> Result<String, GraphError> {
    match node.kind.as_str() {
        "data/get" => {
            let key = node
                .params
                .get("key")
                .as_str()
                .ok_or_else(|| {
                    GraphError::at(
                        node.id.clone(),
                        None,
                        "\"key\" param is required — the field to read (a string)",
                    )
                })?
                .to_string();
            let value = value_expr(graph, nodes_by_id, &node.id, "value", depth)?;
            Ok(format!("{value}[{}]", js_str(&key)))
        }
        "math/compare" => {
            let op_var = node.params.get("op");
            let op = op_var.as_str().ok_or_else(|| {
                GraphError::at(
                    node.id.clone(),
                    None,
                    "\"op\" param is required — one of < > == != <= >=",
                )
            })?;
            if !["<", ">", "==", "!=", "<=", ">="].contains(&op) {
                return Err(GraphError::at(
                    node.id.clone(),
                    None,
                    format!("\"op\" param {op:?} is not a comparison (< > == != <= >=)"),
                ));
            }
            let a = value_expr(graph, nodes_by_id, &node.id, "a", depth)?;
            let b = value_expr(graph, nodes_by_id, &node.id, "b", depth)?;
            Ok(format!("({a} {op} {b})"))
        }
        "data/literal" => {
            // the `value` param, emitted as a JSON literal (JSON literals
            // are JS literals). A literal null is indistinguishable from
            // an absent param through `Var::get` — an unwired pin reads
            // undefined instead.
            let value = node.params.get("value");
            if matches!(value, Var::Null) {
                return Err(GraphError::at(
                    node.id.clone(),
                    None,
                    "\"value\" param is required — the literal to emit (a JSON-shaped Var)",
                ));
            }
            serde_json::to_string(&value)
                .map(|json| format!("({json})"))
                .map_err(|e| GraphError::at(node.id.clone(), None, format!("\"value\" param: {e}")))
        }
        other => Err(GraphError::at(
            node.id.clone(),
            None,
            format!("{other:?} resolved as pure but has no expression form (a vocabulary bug)"),
        )),
    }
}

/// Emit one node's execution into the generated body, walking its exec
/// outs (fan-out = sequential walks; convergence re-walks the shared
/// tail — the Blueprint semantics). `depth` bounds the recursion (the
/// conversion check already rejected cycles; this is belt-and-suspenders).
#[allow(clippy::too_many_arguments)]
fn emit_node(
    graph: &Graph,
    nodes_by_id: &HashMap<&str, &Node>,
    exec: &HashMap<&str, Vec<(&str, &str)>>,
    id: &str,
    declared: &mut HashSet<String>,
    lets: &mut String,
    body: &mut String,
    depth: usize,
) -> Result<(), GraphError> {
    if depth > 128 {
        return Err(GraphError::at(
            id.to_string(),
            None,
            "exec nesting exceeded 128 (a cycle slipped past the check — a compiler bug)",
        ));
    }
    let node = nodes_by_id[id];
    let spec = spec_for(&node.kind).expect("checked");
    let indent = "  ".repeat(depth + 1);

    // declare this node's data-out lets once (function scope)
    if !spec.pure {
        for (pin, _) in &spec.data_outs {
            let name = format!("{id}_{pin}");
            if declared.insert(name.clone()) {
                writeln!(lets, "  let {name};").expect("string write");
            }
        }
    }

    match node.kind.as_str() {
        "web/request" => {
            // the staged request's fields become the data outs
            for pin in ["params", "query", "headers", "body"] {
                writeln!(body, "{indent}{id}_{pin} = req.{pin};").expect("string write");
            }
        }
        "web/respond" => {
            let body_expr = value_expr(graph, nodes_by_id, id, "body", depth)?;
            writeln!(body, "{indent}return {body_expr};").expect("string write");
        }
        "flow/branch" => {
            let cond = value_expr(graph, nodes_by_id, id, "condition", depth)?;
            let mut then_out = String::new();
            let mut else_out = String::new();
            for &(from_pin, target) in exec.get(id).into_iter().flatten() {
                let slot = if from_pin == "then" {
                    &mut then_out
                } else {
                    &mut else_out
                };
                emit_node(
                    graph,
                    nodes_by_id,
                    exec,
                    target,
                    declared,
                    lets,
                    slot,
                    depth + 1,
                )?;
            }
            write!(
                body,
                "{indent}if ({cond}) {{\n{then_out}{indent}}} else {{\n{else_out}{indent}}}\n"
            )
            .expect("string write");
        }
        kind if function_meta(kind).is_some() => {
            // function node: dispatch through the host binding into the
            // function table — the third face of one table
            let meta = function_meta(kind).expect("checked");
            let mut args = Vec::new();
            for i in 0..meta.inputs.len() {
                let expr = value_expr(graph, nodes_by_id, id, &format!("in{i}"), depth)?;
                args.push(expr);
            }
            writeln!(
                body,
                "{indent}{id}_out = {NODE_CALL_HOST}({}, JSON.stringify([{}]));",
                js_str(kind),
                args.join(", "),
            )
            .expect("string write");
        }
        other => {
            return Err(GraphError::at(
                id.to_string(),
                None,
                format!(
                    "{other:?} has no generated form (a vocabulary bug — the checker resolved it)"
                ),
            ));
        }
    }

    // non-branch nodes: walk their exec outs in wire order
    if node.kind != "flow/branch" {
        if let Some(outs) = exec.get(id) {
            for &(from_pin, target) in outs {
                debug_assert_eq!(from_pin, "exec", "non-branch exec outs are 'exec'");
                emit_node(
                    graph,
                    nodes_by_id,
                    exec,
                    target,
                    declared,
                    lets,
                    body,
                    depth + 1,
                )?;
            }
        }
    }
    Ok(())
}
