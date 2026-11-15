//! The graph → Rust compiler (M4) — the native path. The design's
//! promise: prototype in graph ergonomics, derive a typed native
//! artifact. This compiler emits a **self-contained plugin crate** —
//! the hyg-plugin-api contract's two mandated exports around a
//! Var-native route function — so the generated artifact composes with
//! the whole M1 story: `rustc` compiles it,
//! [`load_plugin`](crate::dynamic::ffi::load_plugin) loads it, its
//! route lands in the function table, and hot-swap replaces it like any
//! plugin. **A compiled graph is a plugin.**
//!
//! # The generated shape
//!
//! One route function over `Var`, no JSON on the inside (the native
//! path's win — marshaling happens once at the plugin boundary):
//!
//! ```text
//! fn route(req: &Var) -> Result<Var, String>
//! ```
//!
//! - `web/request` pins become function-scope `let mut` Vars (values
//!   produced in a branch flow out of it, the Blueprint convergence
//!   shape, same as the JS path);
//! - pure nodes inline as Var-native expressions: `data/get` is
//!   `Var::get`, `data/literal` embeds its checked JSON as a `Var`,
//!   `math/compare` calls the generated `__hyg_compare` helper and
//!   yields a `Var::Bool` (numeric when both sides read as numbers,
//!   string otherwise, `false` when neither — JS-loose semantics made
//!   explicit);
//! - `web/respond` is a `return Ok(...)`; a path that never reaches one
//!   falls through to `Ok(Var::Null)` — the JS path's `undefined`
//!   reads-as-Null contract, native;
//! - **function nodes link against project Rust**: the
//!   [`RustVocabulary`] binds a callable name to a Rust PATH
//!   (`greeter::greet_var`) whose signature convention is
//!   `fn(&[Var]) -> Result<Var, String>` — the Blueprint→C++ story:
//!   the graph calls your typed functions, your build declares the
//!   dependency. A binding whose callable also lives in the function
//!   table must carry the SAME signature the table does, or conversion
//!   refuses — the two compilers must never disagree about a graph.
//!
//! # Node markers and span remapping
//!
//! Every node's generated code is preceded by a `// [hyg:node:<id>]`
//! marker line. When the generated source fails to compile (rare —
//! host-API drift, exactly what the design anticipates: a bound path
//! that no longer exists), [`build_generated`] parses rustc's
//! `--> src/lib.rs:line:col` spans, remaps each through the nearest
//! preceding marker, and re-reports the failure as a [`GraphError`]
//! with graph coordinates. The generated source is a build artifact,
//! never hand-edited — so a line number is always a node number.
//!
//! Pure data, no feature gate. Building the generated crate shells out
//! to `cargo` (a Rust toolchain must exist at build time — the native
//! path's tradeoff, the JS path's freedom); the tests drive it for
//! real.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::dynamic::functions::{function_meta, FunctionMeta};
use crate::dynamic::graph::model::{Graph, GraphError};
use crate::dynamic::graph::spec::spec_for;
use crate::dynamic::graph::spec::Checker;
use crate::var::Ty;

/// The generated route function's name inside the crate.
pub const RUST_ROUTE_FN: &str = "route";

/// A function node's native binding: the callable name → a Rust path in
/// a dependency the generated crate declares. The signature convention
/// for bound paths is `fn(&[Var]) -> Result<Var, String>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustBinding {
    /// The Rust path the generated code calls (a dependency-declared
    /// path, e.g. `greeter::greet_var`).
    pub path: String,
    /// The signature the generated code assumes — MUST match the
    /// function table's `FunctionMeta` for the same callable when one
    /// exists (the JS and Rust compilers may never disagree about a
    /// graph).
    pub inputs: Vec<Ty>,
    /// The bound path's output type.
    pub output: Ty,
}

/// The native vocabulary: which callables the Rust path can link
/// against, and through which paths.
#[derive(Debug, Clone, Default)]
pub struct RustVocabulary {
    bindings: HashMap<String, RustBinding>,
}

impl RustVocabulary {
    /// Bind a callable name to a native path.
    pub fn bind(&mut self, callable: impl Into<String>, binding: RustBinding) {
        self.bindings.insert(callable.into(), binding);
    }

    /// The binding for a callable, if one exists.
    #[must_use]
    pub fn binding(&self, callable: &str) -> Option<&RustBinding> {
        self.bindings.get(callable)
    }
}

/// The generated crate's identity and dependency lines.
#[derive(Debug, Clone)]
pub struct GraphCrateSpec {
    /// The plugin id (an ASCII identifier — it namespaces the generated
    /// route callable as `"{plugin_id}.handle"` and names the artifact).
    pub plugin_id: String,
    /// The generated plugin's version.
    pub version: String,
    /// The `hyg` dependency line for the generated Cargo.toml — a path
    /// dep for in-repo builds (`hyg = { path = "…" }`) or a version for
    /// published builds.
    pub hyg_dep: String,
    /// The `hyg-plugin-api` dependency line (path dep for in-repo
    /// builds, version for published ones).
    pub plugin_api_dep: String,
    /// Extra dependencies the bound paths need — `(crate_name,
    /// dependency_line)` pairs appended to the generated Cargo.toml.
    pub extra_deps: Vec<(String, String)>,
}

/// A generated plugin crate: the files to write, ready for
/// [`build_generated`].
#[derive(Debug, Clone)]
pub struct GeneratedCrate {
    /// The crate (and artifact) name.
    pub name: String,
    /// `(relative path, contents)` — `Cargo.toml` and `src/lib.rs`.
    pub files: Vec<(String, String)>,
}

/// Compile a graph document to a native Rust plugin crate — the same
/// conversion contract as the JS path (one checker, one error voice),
/// plus the native vocabulary's resolution.
///
/// # Errors
/// Everything the conversion checker refuses, plus: a function node
/// whose callable has no [`RustBinding`], or whose binding's signature
/// disagrees with the function table's.
pub fn generate(
    graph: &Graph,
    vocab: &RustVocabulary,
    spec: &GraphCrateSpec,
) -> Result<GeneratedCrate, GraphError> {
    // the graph re-runs the ONE checker here — callers pass a typed
    // Graph, never a half-validated one (the JS path's `compile` shares
    // the same pass)
    let mut checker = Checker::new(graph)?;
    checker.check()?;
    resolve_vocabulary(graph, vocab)?;
    let lib_rs = emit_lib(graph, &checker, vocab, spec)?;
    Ok(GeneratedCrate {
        name: spec.plugin_id.clone(),
        files: vec![
            ("Cargo.toml".to_string(), emit_cargo_toml(spec)),
            ("src/lib.rs".to_string(), lib_rs),
        ],
    })
}

/// Every function node must bind, and a binding must agree with the
/// function table when the callable is registered there too.
fn resolve_vocabulary(graph: &Graph, vocab: &RustVocabulary) -> Result<(), GraphError> {
    for node in &graph.nodes {
        let Some(meta) = function_meta(&node.kind) else {
            continue; // not a function node — the checker handled its kind
        };
        let Some(binding) = vocab.binding(&node.kind) else {
            return Err(GraphError::at(
                node.id.clone(),
                None,
                format!(
                    "function node {:?} has no native binding — bind a Rust path for it in the RustVocabulary",
                    node.kind
                ),
            ));
        };
        if binding.inputs != meta.inputs || binding.output != meta.output {
            return Err(GraphError::at(
                node.id.clone(),
                None,
                format!(
                    "native binding for {:?} disagrees with the function table: binding {:?} → {}, table {:?} → {} \
                     — the two compilers must never disagree about a graph",
                    node.kind, binding.inputs, binding.output, meta.inputs, meta.output,
                ),
            ));
        }
    }
    Ok(())
}

/// A Rust source string literal (`{:?}` escapes it correctly).
fn rs_str(s: &str) -> String {
    format!("{s:?}")
}

fn emit_cargo_toml(spec: &GraphCrateSpec) -> String {
    let mut toml = String::new();
    let _ = writeln!(
        toml,
        "# generated by hyg's graph → Rust compiler — build artifact, do not edit"
    );
    let _ = writeln!(toml, "[package]");
    let _ = writeln!(toml, "name = {:?}", spec.plugin_id);
    let _ = writeln!(toml, "version = {:?}", spec.version);
    let _ = writeln!(toml, "edition = \"2021\"");
    let _ = writeln!(toml);
    let _ = writeln!(toml, "[lib]");
    let _ = writeln!(toml, "crate-type = [\"cdylib\"]");
    let _ = writeln!(toml);
    // the generated crate is its own workspace root — it may live inside
    // the host project's directory tree and must not join its workspace
    let _ = writeln!(toml, "[workspace]");
    let _ = writeln!(toml);
    let _ = writeln!(toml, "[dependencies]");
    let _ = writeln!(toml, "{}", spec.hyg_dep);
    let _ = writeln!(toml, "{}", spec.plugin_api_dep);
    let _ = writeln!(toml, "abi_stable = \"0.11\"");
    let _ = writeln!(toml, "serde_json = \"1\"");
    for (name, line) in &spec.extra_deps {
        let _ = writeln!(toml, "{name} = {line}");
    }
    toml
}

/// The generated `src/lib.rs`: contract exports, support helpers, the
/// route function (a marker before every node's code).
fn emit_lib(
    graph: &Graph,
    checker: &Checker<'_>,
    vocab: &RustVocabulary,
    spec: &GraphCrateSpec,
) -> Result<String, GraphError> {
    let mut lib = String::new();
    let _ = writeln!(
        lib,
        "// generated by hyg's graph → Rust compiler — build artifact, do not edit"
    );
    let _ = writeln!(lib, "// graph: {:?}", graph.name);
    let _ = writeln!(lib, "use hyg::Var;");
    let _ = writeln!(
        lib,
        "use abi_stable::std_types::{{RBoxError, ROk, RResult, RString, RVec}};"
    );
    let _ = writeln!(
        lib,
        "use hyg_plugin_api::{{refuse, FunctionSpec, PinTy, PluginManifest, DYNAMIC_ABI}};"
    );
    let _ = writeln!(lib);

    // ---- the contract exports ----
    let _ = writeln!(lib, "#[no_mangle]");
    let _ = writeln!(
        lib,
        "pub extern \"C\" fn hyg_plugin_manifest() -> RString {{"
    );
    let _ = writeln!(lib, "    RString::from(PluginManifest {{");
    let _ = writeln!(lib, "        id: {:?}.to_string(),", spec.plugin_id);
    let _ = writeln!(lib, "        version: {:?}.to_string(),", spec.version);
    let _ = writeln!(lib, "        abi: DYNAMIC_ABI,");
    let _ = writeln!(lib, "        permissions: vec![],");
    let _ = writeln!(lib, "        functions: vec![FunctionSpec {{");
    let _ = writeln!(lib, "            name: \"handle\".to_string(),");
    let _ = writeln!(
        lib,
        "            summary: {:?}.to_string(),",
        format!(
            "the {:?} graph route (generated from a .hyg graph document)",
            graph.name
        )
    );
    // the route's output is whatever its respond emits — Null doubles as
    // the untyped marker for route callables
    let _ = writeln!(lib, "            inputs: vec![PinTy::Object],");
    let _ = writeln!(lib, "            output: PinTy::Null,");
    let _ = writeln!(lib, "        }}],");
    let _ = writeln!(lib, "    }}.to_json())");
    let _ = writeln!(lib, "}}");
    let _ = writeln!(lib);
    let _ = writeln!(lib, "#[no_mangle]");
    let _ = writeln!(lib, "pub extern \"C\" fn hyg_plugin_call(");
    let _ = writeln!(lib, "    name: RString,");
    let _ = writeln!(lib, "    args: RVec<RString>,");
    let _ = writeln!(lib, ") -> RResult<RString, RBoxError> {{");
    let _ = writeln!(lib, "    if name.as_str() != \"handle\" {{");
    let _ = writeln!(
        lib,
        "        return refuse(format!(\"unknown function {{:?}} (offered: handle)\", name.as_str()));"
    );
    let _ = writeln!(lib, "    }}");
    let _ = writeln!(
        lib,
        "    let req_json = args.first().map(RString::as_str).unwrap_or(\"null\");"
    );
    // RResult is not Result — no `?` across the ABI (a residual mismatch)
    let _ = writeln!(
        lib,
        "    let req: Var = match serde_json::from_str(req_json) {{"
    );
    let _ = writeln!(lib, "        Ok(req) => req,");
    let _ = writeln!(
        lib,
        "        Err(e) => return refuse(format!(\"route request: {{e}}\")),"
    );
    let _ = writeln!(lib, "    }};");
    let _ = writeln!(lib, "    match route(&req) {{");
    let _ = writeln!(lib, "        Ok(value) => match __hyg_json(&value) {{");
    let _ = writeln!(lib, "            Ok(text) => ROk(RString::from(text)),");
    let _ = writeln!(lib, "            Err(e) => refuse(e),");
    let _ = writeln!(lib, "        }},");
    let _ = writeln!(lib, "        Err(e) => refuse(e),");
    let _ = writeln!(lib, "    }}");
    let _ = writeln!(lib, "}}");
    let _ = writeln!(lib);

    // ---- support helpers (self-contained: only hyg + serde_json) ----
    let _ = writeln!(lib, "fn __hyg_json(v: &Var) -> Result<String, String> {{");
    let _ = writeln!(lib, "    fn walk(v: &Var) -> Result<(), String> {{");
    let _ = writeln!(lib, "        match v {{");
    let _ = writeln!(
        lib,
        "            Var::F64(f) if !f.is_finite() => Err(format!(\"float {{f}} is not finite — JSON cannot hold it\")),"
    );
    let _ = writeln!(
        lib,
        "            Var::List(l) => {{ for item in l.iter().iter() {{ walk(item)?; }} Ok(()) }}"
    );
    let _ = writeln!(
        lib,
        "            Var::Object(o) => {{ for (_, item) in o.entries().iter() {{ walk(item)?; }} Ok(()) }}"
    );
    let _ = writeln!(lib, "            _ => Ok(()),");
    let _ = writeln!(lib, "        }}");
    let _ = writeln!(lib, "    }}");
    let _ = writeln!(lib, "    walk(v)?;");
    let _ = writeln!(
        lib,
        "    serde_json::to_string(v).map_err(|e| format!(\"value cannot serialize: {{e}}\"))"
    );
    let _ = writeln!(lib, "}}");
    let _ = writeln!(lib);
    let _ = writeln!(
        lib,
        "fn __hyg_compare(a: &Var, b: &Var, op: &str) -> bool {{"
    );
    let _ = writeln!(lib, "    let ord = match (a.as_f64(), b.as_f64()) {{");
    let _ = writeln!(lib, "        (Some(x), Some(y)) => x.partial_cmp(&y),");
    let _ = writeln!(lib, "        _ => match (a.as_str(), b.as_str()) {{");
    let _ = writeln!(lib, "            (Some(x), Some(y)) => Some(x.cmp(y)),");
    let _ = writeln!(lib, "            _ => None,");
    let _ = writeln!(lib, "        }},");
    let _ = writeln!(lib, "    }};");
    let _ = writeln!(lib, "    match ord {{");
    let _ = writeln!(lib, "        Some(o) => match op {{");
    let _ = writeln!(lib, "            \"<\" => o.is_lt(),");
    let _ = writeln!(lib, "            \">\" => o.is_gt(),");
    let _ = writeln!(lib, "            \"==\" => o.is_eq(),");
    let _ = writeln!(lib, "            \"!=\" => o.is_ne(),");
    let _ = writeln!(lib, "            \"<=\" => o.is_le(),");
    let _ = writeln!(lib, "            \">=\" => o.is_ge(),");
    let _ = writeln!(lib, "            _ => false,");
    let _ = writeln!(lib, "        }},");
    let _ = writeln!(lib, "        None => false,");
    let _ = writeln!(lib, "    }}");
    let _ = writeln!(lib, "}}");
    let _ = writeln!(lib);

    // ---- the route function ----
    let _ = writeln!(
        lib,
        "fn {RUST_ROUTE_FN}(req: &Var) -> Result<Var, String> {{"
    );
    // function-scope lets for every reachable non-pure node's data outs
    // (values produced in a branch flow out of it)
    let mut declared: Vec<String> = Vec::new();
    for node in &graph.nodes {
        let spec = spec_for(&node.kind).expect("checked graph: kind resolves");
        if spec.pure || !checker.paths.contains_key(node.id.as_str()) {
            continue;
        }
        for pin_spec in &spec.data_outs {
            let name = format!("{}_{}", node.id, pin_spec.name);
            if !declared.contains(&name) {
                declared.push(name.clone());
                let _ = writeln!(lib, "    let mut {name}: Var = Var::Null;");
            }
        }
    }
    let _ = writeln!(lib);
    let mut body = String::new();
    for id in &graph.entry {
        emit_node(graph, checker, vocab, spec, id, &mut body, 0)?;
    }
    lib.push_str(&body);
    let _ = writeln!(lib, "    Ok(Var::Null)");
    let _ = writeln!(lib, "}}");
    Ok(lib)
}

/// Emit one node's execution, walking exec outs (the same walk as the
/// JS generator — one checker, two generators, same semantics).
fn emit_node(
    graph: &Graph,
    checker: &Checker<'_>,
    vocab: &RustVocabulary,
    spec: &GraphCrateSpec,
    id: &str,
    body: &mut String,
    depth: usize,
) -> Result<(), GraphError> {
    let _ = spec;
    if depth > 128 {
        return Err(GraphError::at(
            id.to_string(),
            None,
            "exec nesting exceeded 128 (a cycle slipped past the check — a compiler bug)",
        ));
    }
    let indent = "    ".repeat(depth + 1);
    let node = graph.node(id).expect("checked graph: node exists");
    let _ = writeln!(body, "{indent}// [hyg:node:{id}]");

    match node.kind.as_str() {
        "web/request" => {
            for pin in ["params", "query", "headers", "body"] {
                let _ = writeln!(body, "{indent}{id}_{pin} = req.get({});", rs_str(pin));
            }
        }
        "web/respond" => {
            let expr = value_expr(graph, checker, id, "body", depth)?;
            let _ = writeln!(body, "{indent}return Ok({expr});");
        }
        "flow/branch" => {
            let cond = value_expr(graph, checker, id, "condition", depth)?;
            let mut then_out = String::new();
            let mut else_out = String::new();
            if let Some(outs) = checker.exec.get(id) {
                for &(from_pin, target, _) in outs {
                    let slot = if from_pin == "then" {
                        &mut then_out
                    } else {
                        &mut else_out
                    };
                    emit_node(graph, checker, vocab, spec, target, slot, depth + 1)?;
                }
            }
            let _ = write!(
                body,
                "{indent}if {cond}.is_truthy() {{\n{then_out}{indent}}} else {{\n{else_out}{indent}}}\n"
            );
        }
        kind => {
            // a function node — the checker + vocabulary resolution
            // guarantee a binding with a table-consistent signature
            let binding = vocab.binding(kind).ok_or_else(|| {
                GraphError::at(
                    id.to_string(),
                    None,
                    format!(
                        "function node {kind:?} has no native binding (a checker bug — resolve_vocabulary passed it)"
                    ),
                )
            })?;
            let meta_inputs = function_meta(kind)
                .map(|m: FunctionMeta| m.inputs)
                .unwrap_or_default();
            let mut args = Vec::new();
            for i in 0..meta_inputs.len() {
                let expr = value_expr(graph, checker, id, &format!("in{i}"), depth)?;
                args.push(expr);
            }
            let _ = writeln!(
                body,
                "{indent}{id}_out = {}(&[{}])?;",
                binding.path,
                args.join(", "),
            );
        }
    }

    if node.kind != "flow/branch" {
        if let Some(outs) = checker.exec.get(id) {
            for &(_, target, _) in outs {
                emit_node(graph, checker, vocab, spec, target, body, depth + 1)?;
            }
        }
    }
    Ok(())
}

/// The Var-native expression for a data input: a let reference for
/// non-pure sources, an inlined expression for pure ones. Unwired pins
/// read `Var::Null` (`is_truthy()` false — the JS path's `undefined`
/// semantics, made explicit).
fn value_expr(
    graph: &Graph,
    checker: &Checker<'_>,
    target: &str,
    pin: &str,
    depth: usize,
) -> Result<String, GraphError> {
    let wire = graph
        .wires
        .iter()
        .find(|w| w.to == target && w.to_pin == pin);
    let Some(wire) = wire else {
        return Ok("Var::Null".to_string());
    };
    let src = graph.node(&wire.from).expect("checked graph: node exists");
    let src_spec = spec_for(&src.kind).expect("checked graph: kind resolves");
    if src_spec.pure {
        if depth > 64 {
            return Err(GraphError::at(
                src.id.clone(),
                wire.from_pin.clone(),
                "pure-node expression nesting exceeded 64 (a cycle slipped past the check — a compiler bug)",
            ));
        }
        pure_expr(graph, checker, src, depth + 1)
    } else {
        Ok(format!("{}_{}", src.id, wire.from_pin))
    }
}

/// The pure node's inlined Var-native expression (params were validated
/// at conversion — this trusts the checker).
fn pure_expr(
    graph: &Graph,
    checker: &Checker<'_>,
    node: &crate::dynamic::graph::model::Node,
    depth: usize,
) -> Result<String, GraphError> {
    match node.kind.as_str() {
        "data/get" => {
            let key = node.params.get("key");
            let key = key.as_str().expect("checked: key param");
            let value = value_expr(graph, checker, &node.id, "value", depth)?;
            Ok(format!("{value}.get({})", rs_str(key)))
        }
        "math/compare" => {
            let op = node.params.get("op");
            let op = op.as_str().expect("checked: op param");
            let a = value_expr(graph, checker, &node.id, "a", depth)?;
            let b = value_expr(graph, checker, &node.id, "b", depth)?;
            // the pin's declared type is Bool — the expression yields a
            // Var::Bool, so branch conditions type-check uniformly
            Ok(format!(
                "Var::from(__hyg_compare(&{a}, &{b}, {}))",
                rs_str(op)
            ))
        }
        "data/literal" => {
            let value = node.params.get("value");
            let json = serde_json::to_string(&value)
                .expect("checked: the literal's JSON (fidelity walked at conversion)");
            Ok(format!(
                "serde_json::from_str::<Var>({}).expect(\"graph literal (checked at conversion)\")",
                rs_str(&json)
            ))
        }
        other => Err(GraphError::at(
            node.id.clone(),
            None,
            format!(
                "{other:?} resolved as pure but has no Rust expression form (a vocabulary bug)"
            ),
        )),
    }
}

/// Write the generated crate to `workspace/<plugin_id>/` and build it
/// with `cargo` into `target_dir`. On success returns the built
/// library's path; on failure the diagnostics are remapped through the
/// node markers into a [`GraphError`] — the design's "rare, only
/// host-API drift" path, reported in graph coordinates.
///
/// # Errors
/// The build failure, remapped; or filesystem errors carrying the path.
pub fn build_generated(
    generated: &GeneratedCrate,
    workspace: &Path,
    target_dir: &Path,
) -> Result<PathBuf, GraphError> {
    let crate_dir = workspace.join(&generated.name);
    for (rel, contents) in &generated.files {
        let path = crate_dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                GraphError::at(
                    None,
                    None,
                    format!("cannot create {}: {e}", parent.display()),
                )
            })?;
        }
        std::fs::write(&path, contents).map_err(|e| {
            GraphError::at(None, None, format!("cannot write {}: {e}", path.display()))
        })?;
    }
    let manifest = crate_dir.join("Cargo.toml");
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .arg("build")
        .arg("-q")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--target-dir")
        .arg(target_dir)
        .output()
        .map_err(|e| GraphError::at(None, None, format!("cargo spawn: {e}")))?;
    if out.status.success() {
        return Ok(target_dir.join("debug").join(format!(
            "{}{}{}",
            std::env::consts::DLL_PREFIX,
            generated.name,
            std::env::consts::DLL_SUFFIX
        )));
    }

    // the build failed — remap the spans through the node markers
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let source = generated
        .files
        .iter()
        .find(|(p, _)| p == "src/lib.rs")
        .map(|(_, c)| c.as_str())
        .unwrap_or("");
    Err(remap_failure(&stderr, source))
}

/// Parse the diagnostics for `--> path:line:col` spans, map the first
/// one to the nearest preceding `// [hyg:node:<id>]` marker in the
/// source, and report the failure as a `GraphError` (or the raw
/// diagnostics when nothing maps).
fn remap_failure(diagnostics: &str, source: &str) -> GraphError {
    let markers: Vec<(usize, String)> = source
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            line.trim()
                .strip_prefix("// [hyg:node:")
                .and_then(|rest| rest.strip_suffix(']'))
                .map(|id| (i + 1, id.to_string()))
        })
        .collect();

    let mut first_span: Option<usize> = None;
    for line in diagnostics.lines() {
        let Some(rest) = line.trim().strip_prefix("--> ") else {
            continue;
        };
        // `src/lib.rs:42:9`
        let mut parts = rest.rsplitn(3, ':');
        let _col = parts.next();
        let line_no = parts.next().and_then(|l| l.parse::<usize>().ok());
        let file = parts.next().unwrap_or_default();
        if let Some(line_no) = line_no {
            if file.ends_with("lib.rs") && first_span.is_none() {
                first_span = Some(line_no);
            }
        }
    }

    let Some(line_no) = first_span else {
        return GraphError::at(
            None,
            None,
            format!("generated crate failed to build (no remappable span):\n{diagnostics}"),
        );
    };
    let node = markers
        .iter()
        .filter(|(marker_line, _)| *marker_line <= line_no)
        .max_by_key(|(marker_line, _)| *marker_line)
        .map(|(_, id)| id.clone());
    let summary: String = diagnostics
        .lines()
        .filter(|l| l.starts_with("error"))
        .take(3)
        .collect::<Vec<_>>()
        .join("; ");
    GraphError {
        node,
        pin: None,
        message: format!(
            "generated crate failed to build at src/lib.rs:{line_no}: {summary} \
             (host-API drift — the bound paths must exist in the declared dependencies)"
        ),
        flow: Vec::new(),
    }
}
