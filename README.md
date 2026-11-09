# Hydrargyros ☿

**hyg** is a Rust framework for high-level, JavaScript-like ergonomics for
prototyping without sacrificing speed. Dynamic, prototype-based values with
script-quality sugar on the surface; borrow-checked, multithreaded, serde
native underneath.

The name: *hydrargyrum* — Greek, "water-silver" — the element mercury, **Hg**;
the planet and the swift messenger god, fitting the fast-prototyping goal.
The project is Hydrargyros; the crate and namespace are `hyg` (short, and
distinct from `hg`, which is the Mercurial VCS binary).

## The idea

- **Values like JavaScript.** `Var` is a COW dynamic value — objects
  (insertion-ordered, prototype chains), ordered lists, template strings, the
  `o!{}` / `a![]` literal macros, `each`/`map`/`filter`/`find`/`join`.
- **Explicit where JavaScript is implicit.** Reads and writes go through
  `.get()` / `.put()` / `.set()` — no assignment-conversion footgun; typed
  reads via `Into<…>` / `as_*()`; `Null` as a literal. Borrow checking does
  the work implicit casts papered over.
- **Shared by default.** Payloads are `Arc<Mutex>`, so the whole value graph
  is `Send + Sync`: the same `Var` crosses threads, async tasks, and web
  handlers, and the `VarRef` proxy mutates the original in place.
- **serde is the serialization layer.** One `Serialize`/`Deserialize` impl on
  `Var` — JSON, CBOR, any serde format, for free. Function values ride as
  `{"__func__": id}` handles through a process-global registry.
- **Prototype fast, ship typed.** `#[derive(FromVar)]` turns a `Var` object
  into a strict, path-tracked struct (`"user.stats.score": expected int,
  found string`) for the hot loop.

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

// write through the shared object
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
  `#[derive(FromVar)]` (the `hyg-derive` proc-macro crate). *Done.*
- **Web** (`web` feature) — an expressjs-like `Server` on axum + tokio: all
  verbs, `:param` and trailing-`*` patterns, WebSocket routes, static mounts
  with mime detection, an `on_error` hook, streamed bodies (`Response::stream`,
  `route_stream`), HTTPS via the `sslCert`/`sslKey` config pair, and graceful,
  restartable `start()`/`stop()`. Always on (pure data, no feature gate): the
  `html` HTML5 builder + parser, the `css` parser, and the file-backed
  `store` with the `database` facade (`$set`/`$unset`/`$inc`, `$in` queries,
  Mongo-shaped ids). *Done.*
- **Game** — a headless-first engine: the pump frame (poll events → sort +
  init → priority-ordered `update(dt)` → draw → frame cap), priority-tiered
  components, the entity/transform scene tree, and registered window backends
  — `headless` (built in), `winit` (classic desktop window), the **streaming
  window** (`stream` feature: the render buffer becomes a browser page, and
  browser keys/mouse/wheel come back as engine events), a borderless
  fullscreen native view, and a tray glyph (`tray` feature). Physics runs on
  rapier3d behind a `PhysicsWorldBackend` trait with a no-op `none` fallback
  (`physics` feature). Rendering is an offscreen wgpu renderer whose pixel
  tests assert actual PNGs (`wgpu` feature): quad + depth-tested mesh
  pipelines, procedural or glTF models (`gltf` feature), textures
  (`image` feature), eight lights shaded with the KHR punctual math, ortho +
  perspective cameras, and an orbit controller. `game::tween` ships
  easings/timers; audio runs on rodio behind an `AudioBackend` trait with a
  recorder `none` backend (`audio` feature). Scripting goes through the v12
  interpreter seam (`script` feature): `ScriptedComponent` dispatches
  lifecycle hooks to script functions over a JSON bridge. *Done.*
- **Derive** — `FromVar`, re-exported as `hyg::FromVar`. *Done.*

## Examples

`examples/` has a runnable tour — `headless_game` (engine loop, no window),
`game_window` / `game_stream` / `game_world` (window, browser streaming,
physics + camera), `game_sandbox` (everything at once: physics, lights,
scripting, audio, tween, orbit camera), `game_gltf` (glTF through the
perspective path), `web_demo` / `web_site` (routes, HTML builder, store,
WebSockets). See the README table in `examples/` for the full index.

## Documentation

- [docs/roadmap.md](docs/roadmap.md) — the plan and the architecture-decision
  record (why `Arc<Mutex>`, why serde, why not Bevy).
- [docs/gold-lineage.md](docs/gold-lineage.md) — the heritage record: where
  the design descended from and which contracts mirror it. Not needed to
  learn hyg.
- [docs/platforms.md](docs/platforms.md) — platform support matrix.
- Design drafts (for review): [Hydrargyros Studio](docs/game-editor-design.md)
  (game engine & editor), [Hydrargyros Press](docs/web-editor-design.md)
  (WYSIWYG website/API editor), and the
  [`dynamic` module](docs/dynamic-design.md) (FFI, scripts, visual
  programming).

## Build & test

```
cargo test                  # core tests
cargo test --all-features   # full suite: web, physics, wgpu, stream, gltf, winit, tray
cargo clippy --all-targets --all-features
```

Features: `web`, `physics`, `wgpu`, `winit`, `tray`, `stream`, `gltf`,
`image`, `script`, `audio`. New heavy dependencies go behind a feature.

## License

MIT
