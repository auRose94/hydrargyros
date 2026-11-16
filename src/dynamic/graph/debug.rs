//! The graph debugger — breakpoints on nodes, watches on pins, and
//! single-stepping over exec wires. **One implementation for both
//! modules** (the design's promise): a web route's handler and a game
//! frame's component are both "handlers over `Var`", and both debug
//! through this seam.
//!
//! # How it works
//!
//! A [`DebugRun`] is an *interpreter* over a validated graph — the
//! compiled paths (JS, Rust) exist for speed; a debug execution walks
//! the same graph through the same seams: the shared checker validated
//! it, function nodes call the SAME function table (`call_function`),
//! and pure nodes evaluate with the canonical Var-native semantics
//! ([`native_compare`] — the generated Rust helper must match it, and
//! the cross-path tests check that all three agree).
//!
//! The run is a state machine driven from two sides:
//!
//! - the **executing side** ([`DebugRun::drive`]) advances one segment —
//!   to the next pause or the finish. Non-blocking: the game pump paces
//!   a graph *per frame* by driving it from a component's `update`, and
//!   a route handler that wants a hard freeze calls
//!   [`DebugRun::run_blocking`], which parks the request's thread at
//!   every pause — a breakpoint freezes the frame;
//! - the **debugger side** ([`DebugRun::wait_event`]) observes: at a
//!   pause it reads [`DebugRun::watch`] (any pin's live value,
//!   including the staged request's), then [`DebugRun::resume`] or
//!   [`DebugRun::step`]. Runtime errors carry the *actual walked path*
//!   as their flow chain — the graph's stack trace, ground truth
//!   instead of a prediction.
//!
//! Active runs register in a process-global registry
//! ([`register_run`]/[`active_runs`]) — the seam an admin console or
//! the Studio panel attaches through. Feature-free: pure std
//! synchronization over pure data.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use crate::dynamic::functions::call_function;
use crate::dynamic::graph::model::{Graph, GraphError};
use crate::dynamic::graph::spec::{spec_for, Checker};
use crate::var::Var;

/// The canonical pure-node comparison — numeric when both sides read as
/// numbers, lexical when both are strings, `false` when neither. The
/// generated Rust helper (`__hyg_compare`) must match this exactly, and
/// the JS path's loose semantics must agree — the cross-path tests
/// check it.
pub(crate) fn native_compare(a: &Var, b: &Var, op: &str) -> bool {
    let ord = match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x.partial_cmp(&y),
        _ => match (a.as_str(), b.as_str()) {
            (Some(x), Some(y)) => Some(x.cmp(y)),
            _ => None,
        },
    };
    match ord {
        Some(o) => match op {
            "<" => o.is_lt(),
            ">" => o.is_gt(),
            "==" => o.is_eq(),
            "!=" => o.is_ne(),
            "<=" => o.is_le(),
            ">=" => o.is_ge(),
            _ => false,
        },
        None => false,
    }
}

/// Why a run paused at a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PauseReason {
    /// The node is in the run's breakpoint set.
    Breakpoint,
    /// A debugger requested one node of progress.
    Step,
}

/// What a waiting debugger observes.
#[derive(Debug, Clone)]
pub enum DebugEvent {
    /// The run paused before executing a node (its pins are watchable;
    /// nothing of this node has run yet).
    Paused { node: String, reason: PauseReason },
    /// The run finished — a respond emitted its value, the exec walk
    /// exhausted without one (Null), or a node call failed (the error's
    /// flow chain is the path actually walked).
    Finished { result: Result<Var, GraphError> },
}

/// One drive segment's outcome (the executing side's view).
#[derive(Debug, Clone)]
pub enum DriveOutcome {
    /// The run paused before a node — wait for a command or return (the
    /// pump continues; the graph waits between frames).
    Paused { node: String, reason: PauseReason },
    /// The run finished (see [`DebugEvent::Finished`]).
    Finished { result: Result<Var, GraphError> },
}

#[derive(Debug, Clone)]
enum Phase {
    /// Armed and executing (the entry walk is on the stack). `skip` is
    /// the node a pause just happened at — resuming must not re-stop on
    /// it (a breakpoint fires once per arrival), and `step_once` pauses
    /// again at the NEXT entry.
    Running {
        step_once: bool,
        skip: Option<String>,
    },
    /// Paused before a node, waiting for a debugger command.
    Paused { node: String, reason: PauseReason },
    /// Finished — the result is final.
    Done,
    /// Aborted by a debugger (a stuck run must not hold a frame).
    Aborted,
}

struct RunState {
    phase: Phase,
    breakpoints: HashSet<String>,
    /// Node → pin → value: every executed non-pure node's data outs
    /// (pure pins compute on demand — see [`DebugRun::watch`]).
    store: HashMap<String, HashMap<String, Var>>,
    /// The exec chain actually walked, entry first (the runtime flow
    /// chain's ground truth).
    path: Vec<String>,
    /// Pending exec targets, next-pop first (wire order preserved).
    stack: Vec<String>,
    /// The staged input (the request, the frame's payload).
    input: Var,
    /// The final result once Done.
    result: Option<Result<Var, GraphError>>,
}

/// One debug execution of one graph. Shared freely (`Arc`) — the
/// executing side and the debugger side hold clones of the same run.
pub struct DebugRun {
    graph: Graph,
    /// Exec adjacency in wire order: node → [(from_pin, target)].
    exec: HashMap<String, Vec<(String, String)>>,
    state: Mutex<RunState>,
    signal: Condvar,
}

/// The stop decision for the node at the stack's front — NONE means
/// execute it. The paused node itself is never re-stopped (its pause
/// already happened; `skip` carries it), and it stays QUEUED on a stop:
/// resume executes it.
fn stop_reason_for(
    id: &str,
    skip: &Option<String>,
    step_once: bool,
    breakpoints: &HashSet<String>,
) -> Option<PauseReason> {
    if skip.as_deref() == Some(id) {
        None
    } else if breakpoints.contains(id) {
        Some(PauseReason::Breakpoint)
    } else if step_once {
        Some(PauseReason::Step)
    } else {
        None
    }
}

impl DebugRun {
    /// Validate the graph (the ONE checker) and arm a run. The input is
    /// the staged payload — a route's request, a frame's context.
    ///
    /// # Errors
    /// Everything the conversion checker refuses.
    pub fn start(
        graph: &Graph,
        input: Var,
        breakpoints: Vec<String>,
    ) -> Result<Arc<DebugRun>, GraphError> {
        let mut checker = Checker::new(graph)?;
        checker.check()?;

        // exec adjacency in wire order (pure sources excluded — they have
        // no exec pins by construction)
        let mut exec: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for wire in &graph.wires {
            let from_kind = &graph.node(&wire.from).expect("checked graph: node").kind;
            let Some(spec) = spec_for(from_kind) else {
                continue;
            };
            if spec.exec_outs.contains(&wire.from_pin) {
                exec.entry(wire.from.clone())
                    .or_default()
                    .push((wire.from_pin.clone(), wire.to.clone()));
            }
        }

        let state = RunState {
            phase: Phase::Running {
                step_once: false,
                skip: None,
            }, // run mode — see start_stepping
            breakpoints: breakpoints.into_iter().collect(),
            store: HashMap::new(),
            path: Vec::new(),
            stack: graph.entry.clone(),
            input,
            result: None,
        };
        Ok(Arc::new(DebugRun {
            graph: graph.clone(),
            exec,
            state: Mutex::new(state),
            signal: Condvar::new(),
        }))
    }

    /// Arm a run that pauses before EVERY node — the per-frame pacing
    /// shape: the game pump drives it node-by-node from a component's
    /// `update` ([`Self::drive`], then [`Self::step`] to arm the next).
    ///
    /// # Errors
    /// Everything the conversion checker refuses.
    pub fn start_stepping(
        graph: &Graph,
        input: Var,
        breakpoints: Vec<String>,
    ) -> Result<Arc<DebugRun>, GraphError> {
        let run = Self::start(graph, input, breakpoints)?;
        {
            let mut st = run.state.lock().expect("debug run state");
            st.phase = Phase::Running {
                step_once: true,
                skip: None,
            };
        }
        Ok(run)
    }

    /// Whether the run is paused (a debugger checks before commanding).
    #[must_use]
    pub fn is_paused(&self) -> bool {
        matches!(
            self.state.lock().expect("debug run state").phase,
            Phase::Paused { .. }
        )
    }

    /// The staged input (visible at every pause — the design's "the
    /// staged Var request is visible").
    #[must_use]
    pub fn input(&self) -> Var {
        self.state.lock().expect("debug run state").input.clone()
    }

    /// The exec chain walked so far (entry first). At a pause this
    /// EXCLUDES the paused node (it has not executed yet).
    #[must_use]
    pub fn path(&self) -> Vec<String> {
        self.state.lock().expect("debug run state").path.clone()
    }

    /// Arm a breakpoint mid-run (applies from the next node entry on).
    pub fn add_breakpoint(&self, node: impl Into<String>) {
        let mut st = self.state.lock().expect("debug run state");
        st.breakpoints.insert(node.into());
    }

    pub fn remove_breakpoint(&self, node: &str) {
        let mut st = self.state.lock().expect("debug run state");
        st.breakpoints.remove(node);
    }

    /// Continue to the next breakpoint or the finish. `true` when a
    /// paused run was actually resumed.
    pub fn resume(&self) -> bool {
        let mut st = self.state.lock().expect("debug run state");
        if let Phase::Paused { node, .. } = st.phase.clone() {
            // the paused node stays queued — resume executes it, and the
            // breakpoint must not re-fire on this arrival
            st.phase = Phase::Running {
                step_once: false,
                skip: Some(node),
            };
            self.signal.notify_all();
            true
        } else {
            false
        }
    }

    /// Advance exactly one node, then pause again. `true` when a paused
    /// run was actually stepped.
    pub fn step(&self) -> bool {
        let mut st = self.state.lock().expect("debug run state");
        if let Phase::Paused { node, .. } = st.phase.clone() {
            // one node of progress: execute the paused node, pause again
            // at the next entry
            st.phase = Phase::Running {
                step_once: true,
                skip: Some(node),
            };
            self.signal.notify_all();
            true
        } else {
            false
        }
    }

    /// Pause a RUNNING run at its next node entry — pause-anywhere, no
    /// pre-declared breakpoint needed. The armed stop applies from the
    /// next entry the executing side reaches (mid-`execute` included: the
    /// current node finishes, the next one pauses). `true` when the
    /// request was armed (a paused or finished run is already stopped and
    /// needs no arming).
    pub fn pause(&self) -> bool {
        let mut st = self.state.lock().expect("debug run state");
        if let Phase::Running { skip, .. } = st.phase.clone() {
            st.phase = Phase::Running {
                step_once: true,
                skip,
            };
            true
        } else {
            false
        }
    }

    /// Abort a stuck run (a breakpoint nobody resumes must not hold a
    /// frame or a request thread forever). The executing side unwinds
    /// with an explicit error.
    pub fn abort(&self) {
        let mut st = self.state.lock().expect("debug run state");
        if !matches!(st.phase, Phase::Done) {
            st.phase = Phase::Aborted;
            self.signal.notify_all();
        }
    }

    /// Drive one segment: to the next pause or the finish. NON-blocking
    /// — the game pump calls this from a component's `update` (one node
    /// per frame), the blocking route loop calls it and then parks.
    /// Single-driver convention: one thread drives a run's execution;
    /// debugger threads observe and command.
    ///
    /// # Errors
    /// Aborted runs, misuse (driving while paused without a command),
    /// or the graph having been mutated since validation (a bug-guard
    /// voice — the run owns a validated clone).
    pub fn drive(&self) -> Result<DriveOutcome, GraphError> {
        let mut st = self.state.lock().expect("debug run state");
        match st.phase.clone() {
            Phase::Done => {
                let result = st.result.clone().expect("Done carries a result");
                Ok(DriveOutcome::Finished { result })
            }
            Phase::Aborted => Err(GraphError::at(
                None,
                None,
                "debug run aborted by its debugger",
            )),
            Phase::Paused { node, reason } => Err(GraphError::at(
                node,
                None,
                format!("the run is paused ({reason:?}) — resume() or step() before driving"),
            )),
            Phase::Running { step_once, skip } => {
                let Some(id) = st.stack.first().cloned() else {
                    // the exec walk exhausted without a respond — the
                    // generated paths' Ok(Null) fall-through, native
                    st.phase = Phase::Done;
                    st.result = Some(Ok(Var::Null));
                    self.signal.notify_all();
                    return Ok(DriveOutcome::Finished {
                        result: Ok(Var::Null),
                    });
                };

                // pause BEFORE executing the node (its pins are watchable,
                // nothing of it has run). The node STAYS QUEUED on a stop —
                // resume/step executes it and must not re-stop here.
                let reason = stop_reason_for(&id, &skip, step_once, &st.breakpoints);
                if let Some(reason) = reason {
                    st.phase = Phase::Paused {
                        node: id.clone(),
                        reason: reason.clone(),
                    };
                    self.signal.notify_all();
                    return Ok(DriveOutcome::Paused { node: id, reason });
                }
                st.stack.remove(0);
                st.phase = Phase::Running {
                    step_once,
                    skip: None,
                };

                // execute OUTSIDE the decision — execute re-locks for its
                // own mutations and never holds the state across a node
                // call
                drop(st);
                match self.execute(&id) {
                    Ok(finished) => {
                        let st = self.state.lock().expect("debug run state");
                        if finished {
                            let result = st.result.clone().expect("finish carries a result");
                            return Ok(DriveOutcome::Finished { result });
                        }
                        drop(st);
                        // continue driving? NO — one segment per drive:
                        // per-frame pacing steps node-by-node even without
                        // breakpoints. The blocking runner loops drive().
                        self.drive_segment_continue()
                    }
                    Err(e) => {
                        // a node call failed — the run finishes with the
                        // error, its flow chain the path actually walked
                        let mut st = self.state.lock().expect("debug run state");
                        st.phase = Phase::Done;
                        let mut flow = st.path.clone();
                        flow.push(id.clone());
                        flow.reverse();
                        let err = GraphError {
                            node: Some(id.clone()),
                            pin: None,
                            message: e.message,
                            flow,
                        };
                        st.result = Some(Err(err.clone()));
                        self.signal.notify_all();
                        Ok(DriveOutcome::Finished { result: Err(err) })
                    }
                }
            }
        }
    }

    /// The rest of the current segment after a node executed: if the
    /// next node is unattended (no breakpoint, no step armed), keep
    /// going WITHIN this drive — a segment runs to the next pause or
    /// the finish, so a graph without breakpoints completes in one
    /// drive. (Mutual recursion with [`Self::execute`] through this
    /// tail — bounded by the checker's cycle refusals.)
    fn drive_segment_continue(&self) -> Result<DriveOutcome, GraphError> {
        let mut st = self.state.lock().expect("debug run state");
        match st.phase.clone() {
            Phase::Done => {
                let result = st.result.clone().expect("Done carries a result");
                Ok(DriveOutcome::Finished { result })
            }
            Phase::Running { step_once, skip } => {
                let Some(id) = st.stack.first().cloned() else {
                    st.phase = Phase::Done;
                    st.result = Some(Ok(Var::Null));
                    self.signal.notify_all();
                    return Ok(DriveOutcome::Finished {
                        result: Ok(Var::Null),
                    });
                };
                let reason = stop_reason_for(&id, &skip, step_once, &st.breakpoints);
                if let Some(reason) = reason {
                    st.phase = Phase::Paused {
                        node: id.clone(),
                        reason: reason.clone(),
                    };
                    self.signal.notify_all();
                    return Ok(DriveOutcome::Paused { node: id, reason });
                }
                st.stack.remove(0);
                st.phase = Phase::Running {
                    step_once,
                    skip: None,
                };
                drop(st);
                match self.execute(&id) {
                    Ok(true) => {
                        let st = self.state.lock().expect("debug run state");
                        Ok(DriveOutcome::Finished {
                            result: st.result.clone().expect("finish carries a result"),
                        })
                    }
                    Ok(false) => self.drive_segment_continue(),
                    Err(e) => {
                        let mut st = self.state.lock().expect("debug run state");
                        st.phase = Phase::Done;
                        let mut flow = st.path.clone();
                        flow.reverse();
                        let err = GraphError {
                            node: Some(id.clone()),
                            pin: None,
                            message: e.message,
                            flow,
                        };
                        st.result = Some(Err(err.clone()));
                        self.signal.notify_all();
                        Ok(DriveOutcome::Finished { result: Err(err) })
                    }
                }
            }
            Phase::Paused { node, reason } => Ok(DriveOutcome::Paused { node, reason }),
            Phase::Aborted => Err(GraphError::at(
                None,
                None,
                "debug run aborted by its debugger",
            )),
        }
    }

    /// The blocking runner: drive until finished, parking at every pause
    /// until a debugger commands. The route handler's shape — a
    /// breakpoint freezes the request's thread (and with the pump, the
    /// frame).
    ///
    /// # Errors
    /// A node call's failure (flow chain = the walked path), or an
    /// aborted run.
    pub fn run_blocking(&self) -> Result<Var, GraphError> {
        loop {
            match self.drive()? {
                DriveOutcome::Finished { result } => return result,
                DriveOutcome::Paused { .. } => self.wait_for_command(),
            }
        }
    }

    /// Block until a paused run receives a command (or aborts). Panic-
    /// safe: the condvar releases the lock while waiting.
    fn wait_for_command(&self) {
        let mut st = self.state.lock().expect("debug run state");
        while matches!(st.phase, Phase::Paused { .. }) {
            st = self.signal.wait(st).expect("debug run state");
        }
    }

    /// A debugger's wait: block (bounded) until the run pauses or
    /// finishes. `None` on timeout.
    #[must_use]
    pub fn wait_event(&self, timeout: Duration) -> Option<DebugEvent> {
        let deadline = Instant::now() + timeout;
        let mut st = self.state.lock().expect("debug run state");
        loop {
            match &st.phase {
                Phase::Paused { node, reason } => {
                    return Some(DebugEvent::Paused {
                        node: node.clone(),
                        reason: reason.clone(),
                    });
                }
                Phase::Done => {
                    return Some(DebugEvent::Finished {
                        result: st.result.clone().expect("Done carries a result"),
                    });
                }
                Phase::Aborted => {
                    return Some(DebugEvent::Finished {
                        result: Err(GraphError::at(None, None, "debug run aborted")),
                    });
                }
                Phase::Running { .. } => {}
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (guard, _timeout) = self
                .signal
                .wait_timeout(st, deadline - now)
                .expect("debug run state");
            st = guard;
        }
    }

    /// A pin's live value at a pause. A data-in resolves its source (a
    /// pure chain computes on demand); a data-out reads the store (a
    /// pure out computes; a not-yet-executed node's out is an explicit
    /// error, never a silent Null).
    ///
    /// # Errors
    /// Unknown node/pin, or an unexecuted non-pure out.
    pub fn watch(&self, node: &str, pin: &str) -> Result<Var, GraphError> {
        let graph_node = self.graph.node(node).ok_or_else(|| {
            GraphError::at(
                Some(node.to_string()),
                Some(pin.to_string()),
                "watch: unknown node (is it in this graph?)",
            )
        })?;
        let spec = spec_for(&graph_node.kind).ok_or_else(|| {
            GraphError::at(
                Some(node.to_string()),
                Some(pin.to_string()),
                "watch: the node's kind stopped resolving (a checker bug)",
            )
        })?;
        if spec.data_ins.iter().any(|p| p.name == pin) {
            return self.resolve_expr(node, pin);
        }
        if spec.data_outs.iter().any(|p| p.name == pin) {
            if spec.pure {
                return self.eval_pure(graph_node);
            }
            let st = self.state.lock().expect("debug run state");
            return st
                .store
                .get(node)
                .and_then(|pins| pins.get(pin))
                .cloned()
                .ok_or_else(|| {
                    GraphError::at(
                        Some(node.to_string()),
                        Some(pin.to_string()),
                        "watch: no value yet — the node has not executed",
                    )
                });
        }
        Err(GraphError::at(
            Some(node.to_string()),
            Some(pin.to_string()),
            "watch: not a data pin (exec pins carry control, not values)",
        ))
    }

    /// The value one data pin resolves to, mirroring the generated
    /// paths: a wired non-pure source is its stored value (Null before
    /// execution — the same let semantics), a pure source computes on
    /// demand, an unwired pin is Null.
    fn resolve_expr(&self, target: &str, pin: &str) -> Result<Var, GraphError> {
        let Some(wire) = self
            .graph
            .wires
            .iter()
            .find(|w| w.to == target && w.to_pin == pin)
        else {
            return Ok(Var::Null);
        };
        let src = self.graph.node(&wire.from).expect("checked graph: node");
        let src_spec = spec_for(&src.kind).expect("checked graph: kind resolves");
        if src_spec.pure {
            self.eval_pure(src)
        } else {
            let st = self.state.lock().expect("debug run state");
            Ok(st
                .store
                .get(&wire.from)
                .and_then(|pins| pins.get(&wire.from_pin))
                .cloned()
                .unwrap_or(Var::Null))
        }
    }

    /// A pure node's value (the canonical Var-native semantics —
    /// `data/get` is `Var::get`, `data/literal` is its checked param,
    /// `math/compare` is [`native_compare`]).
    fn eval_pure(&self, node: &crate::dynamic::graph::model::Node) -> Result<Var, GraphError> {
        match node.kind.as_str() {
            "data/get" => {
                let key = node
                    .params
                    .get("key")
                    .as_str()
                    .expect("checked: key param")
                    .to_string();
                let value = self.resolve_expr(&node.id, "value")?;
                Ok(value.get(&key))
            }
            "math/compare" => {
                let op = node.params.get("op");
                let op = op.as_str().expect("checked: op param").to_string();
                let a = self.resolve_expr(&node.id, "a")?;
                let b = self.resolve_expr(&node.id, "b")?;
                Ok(Var::from(native_compare(&a, &b, &op)))
            }
            "data/literal" => {
                let value = node.params.get("value");
                Ok(value)
            }
            other => Err(GraphError::at(
                Some(node.id.clone()),
                None,
                format!(
                    "{other:?} resolved as pure but has no native evaluation (a vocabulary bug)"
                ),
            )),
        }
    }

    /// Execute one node: store its data outs, walk its exec outs.
    /// Returns whether the run finished (a respond, or a failure — the
    /// caller turns failures into the run's result). Node calls happen
    /// OUTSIDE the state lock.
    fn execute(&self, id: &str) -> Result<bool, GraphError> {
        let node = self.graph.node(id).expect("checked graph: node").clone();
        // the path records the executed node
        {
            let mut st = self.state.lock().expect("debug run state");
            st.path.push(id.to_string());
        }
        match node.kind.as_str() {
            "web/request" => {
                let input = self.input();
                let mut outs = HashMap::new();
                for pin in ["params", "query", "headers", "body"] {
                    outs.insert(pin.to_string(), input.get(pin));
                }
                self.record_outs(id, outs);
                self.push_exec_targets(id, "exec");
                Ok(false)
            }
            "web/respond" => {
                let body = self.resolve_expr(id, "body")?;
                let mut st = self.state.lock().expect("debug run state");
                st.phase = Phase::Done;
                st.result = Some(Ok(body));
                self.signal.notify_all();
                Ok(true)
            }
            "flow/branch" => {
                let cond = self.resolve_expr(id, "condition")?;
                let side = if cond.is_truthy() { "then" } else { "else" };
                self.push_exec_targets(id, side);
                Ok(false)
            }
            kind => {
                // a function node — through the SAME function table the
                // compiled paths dispatch into
                let spec = spec_for(kind).expect("checked graph: kind resolves");
                let mut args = Vec::new();
                for pin in &spec.data_ins {
                    args.push(self.resolve_expr(id, &pin.name)?);
                }
                let out = call_function(kind, &args).map_err(|e| GraphError {
                    node: Some(id.to_string()),
                    pin: None,
                    message: format!("{kind}: {e}"),
                    flow: Vec::new(), // filled by the caller with the walked path
                })?;
                self.record_outs(id, HashMap::from([("out".to_string(), out)]));
                self.push_exec_targets(id, "exec");
                Ok(false)
            }
        }
    }

    fn record_outs(&self, id: &str, outs: HashMap<String, Var>) {
        let mut st = self.state.lock().expect("debug run state");
        st.store.insert(id.to_string(), outs);
    }

    /// Push the chosen exec side's targets so the NEXT pops run in wire
    /// order.
    fn push_exec_targets(&self, id: &str, from_pin: &str) {
        let targets: Vec<String> = self
            .exec
            .get(id)
            .into_iter()
            .flatten()
            .filter(|(pin, _)| pin == from_pin)
            .map(|(_, t)| t.clone())
            .collect();
        let mut st = self.state.lock().expect("debug run state");
        for target in targets.into_iter().rev() {
            st.stack.insert(0, target);
        }
    }
}

// ---- the registry: the seam a debugger client attaches through ----

static DEBUG_RUNS: OnceLock<RwLock<HashMap<u64, Arc<DebugRun>>>> = OnceLock::new();
static DEBUG_ID: AtomicU64 = AtomicU64::new(1);

fn runs() -> &'static RwLock<HashMap<u64, Arc<DebugRun>>> {
    DEBUG_RUNS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register an active run (the route handler's move — the debugger
/// client finds the run through [`active_runs`]). Returns its id.
pub fn register_run(run: Arc<DebugRun>) -> u64 {
    let id = DEBUG_ID.fetch_add(1, Ordering::Relaxed);
    runs().write().expect("debug run registry").insert(id, run);
    id
}

/// Remove a finished run.
pub fn unregister_run(id: u64) {
    runs().write().expect("debug run registry").remove(&id);
}

/// The active runs (id ascending — the registry snapshot a debugger
/// client polls).
#[must_use]
pub fn active_runs() -> Vec<(u64, Arc<DebugRun>)> {
    let mut runs: Vec<(u64, Arc<DebugRun>)> = runs()
        .read()
        .expect("debug run registry")
        .iter()
        .map(|(k, v)| (*k, Arc::clone(v)))
        .collect();
    runs.sort_by_key(|(id, _)| *id);
    runs
}
