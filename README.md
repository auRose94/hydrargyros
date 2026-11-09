# Hydrargyros ☿

A Rust reimagining of [gold](https://github.com/) — a framework whose thesis is
*high-level, JavaScript-like ergonomics for prototyping without sacrificing
speed*. Hydrargyros carries that thesis forward in Rust, where the type system lets
it be safer *and* friendlier.

Hydrargyros is a standalone project. It is **not** a gold branch. Gold (the C++
codebase) is untouched by this repo.

> **Name:** gold is **Au**, hyg is the Greek name for mercury — **Hg**, the symbol ☿ — both chemical-element metals.
> The pairing is a joke *and* a signal.

## The idea

gold's unique move was adding accessible, JS-like syntax on top of a fast core.
Hydrargyros does the same, but honest about where Rust can and can't copy
JavaScript:

- **Faithful ports:** the `Var` value, prototype-based objects, ordered lists,
  the `o!`/`a!` literal macros, template strings, array helpers.
- **Rust-native replacements for what can't copy:** explicit `.get()`/`.put()`
  instead of implicit assignment-conversion; typed reads via `Into<…>`; a `Null`
  literal. These trade a footgun for borrow-checked safety.
- **Better than gold:** serde serialization for free, and (planned) derive
  macros that turn a `Var` into a typed struct for the hot path.

## Quick start

```rust
use hyg::{a, o, Null, Var};

// object / array literals read like JS
let user = o! {
    "name": "bob",
    "age": 30i64,
    "tags": a!["admin", "dev"],
    "stats": o!{ "score": 9i64 },
};

// write through the shared object (gold's v["k"] = ... semantics)
user.at("stats").at("score").put(10i64);
assert_eq!(user.get_path("stats.score").as_i64(), Some(10));

// typed read
let name: String = user.get("name").into();
assert_eq!(name, "bob");

// null is a literal
let missing: Var = user.get("nope");
assert_eq!(missing, Null);
```

Serde round-trips through any format (JSON shown):

```rust
let v: Var = serde_json::from_str(r#"{"a":[1,2,3],"b":true}"#).unwrap();
let json = serde_json::to_string(&v).unwrap();
```

## Modules

- **Core** — `Var` / `Object` / `List` / `VarRef` / sugar / serde / `Func` /
  `#[derive(FromVar)]`. *Done.*
- **Web** — gold's expressjs-like `Server`: all verbs, `:param`/`*` patterns,
  WebSocket routes, static mounts, an error hook, streamed bodies, HTTPS,
  graceful start/stop (axum + tokio, behind the `web` feature) — plus the
  always-on `html` builder/parser, `css` parser, and file-backed `store`
  with the `database` facade. *Done.*
- **Game** — the engine core, headless-first: gold's pump + dispatch,
  priority-tiered components, entity/transform scene tree, backend
  registries, events as `Var`, a rapier3d physics world (behind the
  `physics` feature) with step-and-sync, an offscreen wgpu renderer
  with pixel-verified screenshots (behind the `wgpu` feature), and a
  **streaming window** (behind the `stream` feature) that serves the
  render buffer to a browser and turns browser events into engine events.
  Triangle meshes render through a second pipeline (depth-tested, shaded):
  procedural boxes/spheres/quads or glTF models loaded from `.gltf`/`.glb`
  files (behind the `gltf` feature), drawn at their entity transforms.
  glTF materials' base-color textures sample through the same pipeline
  (behind the `image` feature), and `MeshRenderer::textured` turns a quad
  into a sprite. Lights are gold's `light` component (`point`/`directional`/
  `spot` at entity transforms, KHR punctual shading) gathered by the draw
  pass and shaded per-fragment. The interpreter seam next.

See [docs/roadmap.md](docs/roadmap.md) for the full plan and the architecture
decisions (why `Arc<Mutex>`, why serde, why not Bevy).

## Build & test

```
cargo test                  # core tests
cargo test --all-features   # + web layer
cargo clippy --all-targets
```

## License

MIT
