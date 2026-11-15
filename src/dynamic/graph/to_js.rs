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

use crate::dynamic::functions::function_meta;
use crate::dynamic::graph::model::{Graph, GraphError, Node};
use crate::dynamic::graph::spec::{spec_for, Checker};
use crate::var::Var;

/// The generated route's entry function — what the host calls with the
/// staged request.
pub const ROUTE_FN: &str = "__hyg_handle";
/// The host binding function nodes dispatch through (bound by
/// [`super::route`] with a closure into the function table).
pub const NODE_CALL_HOST: &str = "__hyg_node_call";

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
