# The `dynamic` module — FFI, script engines, and visual programming

**Status: draft for review — not a commitment.** This sketch describes the
third application module: `hyg::dynamic`, the scripting/FFI/visual-
programming layer shared by the web and game modules. Working name
`dynamic` (it's the module that makes hyg *dynamic* — behavior loaded and
changed while the process stays up).

## Vision

hyg's apps are **always up** — a web server that never reboots, a game
whose pump never stops. Behavior has to change *while running*: hot-reload
is not a dev convenience here, it is the deployment story. One module owns
all of it:

| Pillar | What it gives web | What it gives game |
| --- | --- | --- |
| **FFI plugins** (native, trusted) | route handlers, auth hooks as loadable plugins | physics/render backends as plugins |
| **Script engines** (sandboxed) | route handlers as JS (today's v12 seam) | `ScriptedComponent` (today's seam, generalized) |
| **Graphs** (visual programming) | route APIs designed as event graphs | gameplay logic wired as event graphs |
| **Debugger + inspector** | break on a request's node, inspect the staged `Var` | break a node mid-frame (the pump already steps) |

The plugin surface is the ecosystem vector: native FFI and WASM are the two
trust levels a future marketplace needs — signed native plugins for
installed setups, sandboxed graph/WASM plugins for everything shared. The
marketplace itself is deliberately *later*; the formats and trust seams
designed here are what makes it possible without a rewrite.

## The graph — Blueprints, hyg-shaped

A 2D node graph in the Unreal Blueprint spirit: **event nodes** (on
request, on key, on frame, on timer), **flow nodes** (branch, sequence,
loop), **function nodes** (anything callable: built-ins, script functions,
FFI plugin functions), and **data pins** typed by `Var`'s own type system
(`Null`/`Bool`/`I64`/`F64`/`Str`/`List`/`Object`/`Func`).

### The carrier is `.hyg`

The intermediate language **is** a hyg graph document — a serialized `Var`
in the `.hyg` carrier convention ([game-editor-design.md](game-editor-design.md)):
`graph.json.hyg` while authoring (diffable), `.cbor.hyg` when machines
exchange big graphs. Schema sketch:

```json
{
  "graph": "order-flow",
  "format": 1,
  "nodes": [
    { "id": "req1", "type": "web/request", "pos": [80, 120] },
    { "id": "branch1", "type": "flow/branch", "pos": [320, 140],
      "params": { "path": "cart.total > 100" } },
    { "id": "resp1", "type": "web/respond", "pos": [560, 110] }
  ],
  "wires": [
    { "from": "req1", "pin": "exec", "to": "branch1", "pin": "exec_in" },
    { "from": "req1", "pin": "body", "to": "branch1", "pin": "in" }
  ],
  "entry": ["req1"]
}
```

### Conversion IS validation

A graph is **valid if and only if it converts**. Compilers exist for
**Rust** (the native hot path — generate source, feed the real compiler)
and **JavaScript** (the web hot path and the script seam), with
**WebAssembly** as the third target (sandboxed plugins). The carrier stays
authoritative either way — compiled output is a build artifact, never
hand-edited.

This buys the tooling almost for free:

- **Errors carry graph coordinates.** A failed conversion is not a wall of
  Rust errors — the compiler's diagnostics are remapped to
  `GraphError { node, pin, message, flow: Vec<node_id> }`: the node to
  click, the pin to look at, and the **flow chain** — the graph's equivalent
  of a stack trace, walked backwards from the failure to the entry node.
- **Type checking is pin checking.** A wire from an `I64` pin into an
  `Str` pin is a conversion error naming both ends — caught at edit time,
  before any build.
- **Rust errors still surface honestly.** When the generated Rust fails
  (rare — only host-API drift), the build remaps spans through the
  generator's node markers and reports graph coordinates again.

### Debugger + inspector

- **Breakpoints on nodes**, watches on pins, step-over following exec
  wires. Game side: the pump is already steppable (the Studio design's
  pause/step), so a breakpoint freezes the frame; web side: a breakpoint
  freezes one request's handler, with the staged `Var` request visible.
- **The inspector shows every node's descriptor** — type, pins with
  defaults and ranges, and **doc links on everything**: node docs, pin
  docs, and the host functions behind function nodes (descriptor metadata
  carries URLs; the editor opens them inline).
- Break/watches ride the same seam for both modules because both are
  "handlers over `Var`" — the debugger is one implementation.

## FFI plugins — the trusted tier

A native plugin is a shared library with one mandated export:

```c
// returns a plugin manifest — a .json.hyg Var: id, version, ABI version,
// the functions it offers (name, pins/signature), the permissions it wants
char const* hyg_plugin_manifest(void);
```

The host loads it (`libloading`-style `dlopen`/`LoadLibrary`), checks the
ABI version, reads the manifest, and registers each offered function into
the same callable registry script functions live in — **plugins, scripts,
and graph function nodes are three faces of one function table.**

**Hot reload for always-up processes**, the watch-based-stop pattern again:
the new library version loads side-by-side (never over a mapped file), the
function table swaps atomically under a lock, in-flight calls drain against
a grace period, then the old library unloads. A failed load swaps nothing —
the server keeps running the previous version, and the error surfaces in
the admin/console (web) or the console panel (Studio).

Native FFI is **trusted mode**: it can crash the process and sees raw
pointers. The manifest's `permissions` field is honored where host APIs can
enforce it (which registry namespaces a plugin may register into); the real
security boundary for anything less trusted is WASM.

## Script engines — v12 first, registry-open

The `script` feature's v12 seam becomes the first backend of a general
engine registry: `dynamic` defines the `ScriptEngine` trait (new/load/run/
globals/call/register — today's `Script` surface), and `v12` registers as
`"v12"`. `hyg::script::Script` and `ScriptedComponent` keep their APIs —
they route through the registry — so existing code and tests don't move.
Future backends slot in without touching call sites (a Lua, a second JS,
or a WASM runtime are all "engines" that compile to the same Var bridge).

## Module shape

```
src/dynamic/
  engine.rs      # ScriptEngine trait + register/create registry (the seam)
  ffi.rs         # plugin load/manifest/hot-swap (FFI, unsafe — the only
                 # file in the repo allowed `unsafe` outside wgpu backends)
  graph/
    model.rs     # the graph Var schema — pure data, NO feature gate
    to_js.rs     # graph → JS compiler + validation errors
    to_rust.rs   # graph → Rust source compiler + span remapping
    wasm.rs      # graph/WASM plugin target (sandboxed tier)
  debug.rs       # breakpoints/watches/flow-chain traces
```

Feature gates (core stays lean): `dynamic-ffi`, `dynamic-wasm`,
`dynamic-graph-js`, `dynamic-graph-rust`; the graph *model* and the
registry are pure data and always on, like the web siblings.

**Migration:** `script` remains a working feature (v12 loaded through the
registry); `ScriptedComponent`, `Script::new`, and the JSON bridge keep
their contracts. A later release may rename the feature to `dynamic` with
`script` as an alias — no code moves before the registry proves itself.

## Route APIs as event graphs

The web editor's route designer ([web-editor-design.md](web-editor-design.md))
gains the graph canvas: a route is an event graph — **`web/request`** →
flow nodes → **`web/respond`** — visualized exactly like gameplay
Blueprints, because it *is* the same graph format, the same validators,
the same debugger. Auth middleware is a graph; a webhook pipeline is a
graph; a whole API is a folder of graphs with `:param` patterns on the
entry nodes. The admin's test console drives a request through the live
graph (real traffic, not mocks — the existing test philosophy).

## Marketplace seams (designed now, built later)

The plugin manifest is the contract: `id`, `version`, `abi`, `trust:
"native" | "wasm"`, `permissions`, `engines` (which script engines a graph
requires), and a content hash. Publishing, signing, and discovery are
**later releases** — but every artifact a future registry would index
already has its shape, so the ecosystem can arrive without a format break.

## Milestones (proposed)

| Stage | Deliverable | Done when |
| --- | --- | --- |
| M0 | Engine registry | `ScriptEngine` trait + registry; v12 loads through it; `script` feature and all its tests unchanged and green |
| M1 | FFI plugins | manifest load, function registration, hot-swap with drain on a live server; a test plugin built in CI proves the ABI |
| M2 | Graph model | the Var schema round-trips `.json.hyg`; pins/wires validate structurally; feature-free |
| M3 | Graph → JS | routes defined as graphs run end-to-end; every conversion failure reports `{node, pin, flow}` |
| M4 | Graph → Rust | the native path; generated source compiles; span remapping keeps errors in graph coordinates |
| M5 | Debugger + inspector | breakpoints/watches step a live route and a live frame; doc links resolve |
| M6 | WASM tier | a sandboxed plugin compiles and runs; permissions enforced at the host boundary |

## Open questions

- **ABI strategy** — hand-rolled `extern "C"` (as sketched) vs a framing
  crate (`abi_stable`) for richer signatures. Lean C-ABI first: the
  manifest is data, so complex values can cross as JSON/`Var` text and
  only hot paths ever need raw pointers.
- **WASM runtime choice** — wasmtime is the obvious wheel; note v12 already
  pulls cranelift, so the dependency may be partly shared. Decide at M6.
- **Graph → WASM path** — direct emit vs Rust-target compile. The Rust
  path reuses M4's validation; direct emit may win for size. Prototype
  both before promising.
- **Node vocabulary ownership** — web and game each register built-in
  nodes; the vocabulary is a registry, not a fixed enum, so plugins add
  nodes too (a plugin's manifest offering functions makes them graph
  nodes automatically).
- **Graph versioning** — same story as scene schemas: `format` field plus
  per-node migrations as `Var → Var` functions.
