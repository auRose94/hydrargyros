//! Graph routes — a compiled route graph materialized as a [`Script`]
//! and served on a live web server. The M3 deliverable end to end: a
//! route whose behavior IS a graph document, running real traffic
//! through generated JS, function nodes dispatching into the function
//! table (plugins, scripts — whoever registered), conversions validated
//! before anything runs.
//!
//! The compiled graph's [`Script`] is a `Send + Sync` handle over the
//! thread-local session pool, so each tokio worker materializes its own
//! v12 session on first use (the scripted-component design, reused).
//! The handle's facade serializes concurrent calls on one route
//! (`Arc<Mutex<Script>>`) — correctness first; per-worker session
//! fan-out is the pool's job once a route warrants it.
//!
//! Failure shape: a graph that does not convert never materializes
//! (the [`RouteError::Conversion`] carries the graph coordinates); a
//! node call failing at runtime (a plugin error, a script throw) fails
//! the request loud — 500 with the message, no silent nulls.

use std::fmt;
use std::sync::{Arc, Mutex};

use crate::dynamic::functions::call_function;
use crate::dynamic::graph::model::Graph;
use crate::dynamic::graph::to_js::{compile, NODE_CALL_HOST, ROUTE_FN};
use crate::script::{Script, ScriptError};
use crate::var::Var;
use crate::web::Response;

/// Why a graph route failed to build.
#[derive(Debug)]
pub enum RouteError {
    /// The graph did not convert — the [`GraphError`] carries the
    /// `{node, pin, flow}` coordinates to click.
    Conversion(crate::dynamic::graph::model::GraphError),
    /// The generated JS failed to materialize. The compiler's output is
    /// machine-generated from a checked graph, so this is a compiler bug
    /// if it ever happens — it fails loud, never silent.
    Script(ScriptError),
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouteError::Conversion(e) => write!(f, "route graph did not convert: {e}"),
            RouteError::Script(e) => write!(f, "route graph failed to materialize: {e}"),
        }
    }
}

impl std::error::Error for RouteError {}

/// A route whose behavior is a graph document. Clone-free sharing: the
/// [`Script`] handle rides an `Arc<Mutex<…>>` so the returned handler
/// closure owns its route.
pub struct GraphRoute {
    script: Arc<Mutex<Script>>,
}

impl GraphRoute {
    /// Convert the graph and materialize it as a script. Conversion
    /// failures return before anything runs.
    ///
    /// # Errors
    /// [`RouteError::Conversion`] for graph-coordinate failures,
    /// [`RouteError::Script`] for materialization (a codegen bug's
    /// shape).
    pub fn new(graph: &Graph) -> Result<GraphRoute, RouteError> {
        let compiled = compile(graph).map_err(RouteError::Conversion)?;
        let mut script = Script::new(&compiled.js).map_err(RouteError::Script)?;
        script
            .register_internal(NODE_CALL_HOST, |args: &[Var]| {
                // the generated call: (callable-name, JSON-args-array)
                let name = args
                    .first()
                    .and_then(Var::as_str)
                    .ok_or_else(|| ScriptError::type_err("node call: missing callable name"))?;
                let args_json = args.get(1).and_then(Var::as_str).unwrap_or("[]");
                let vars: Vec<Var> = serde_json::from_str(args_json)
                    .map_err(|e| ScriptError::type_err(format!("node call args: {e}")))?;
                call_function(name, &vars)
            })
            .map_err(RouteError::Script)?;
        Ok(GraphRoute {
            script: Arc::new(Mutex::new(script)),
        })
    }

    /// The web handler: staged request `Var` in, the graph's response
    /// mapped onto a [`Response`] — `Str` as text, `Null` as 204, any
    /// other value as JSON. A failing node call is a loud 500.
    pub fn handler(&self) -> impl Fn(Var) -> Response {
        let script = Arc::clone(&self.script);
        move |req| {
            let mut script = script.lock().expect("graph route script");
            match script.call(ROUTE_FN, &[req]) {
                Ok(Var::Null) => Response::text("").status(204),
                Ok(Var::Str(text)) => Response::text(&text),
                Ok(value) => Response::json(value),
                Err(e) => Response::text(format!("{e}")).status(500),
            }
        }
    }
}
