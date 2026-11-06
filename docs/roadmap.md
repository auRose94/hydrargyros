# Hydrargyros ☿ roadmap

Hydrargyros is a standalone Rust project: a reimagining of **gold** (the C++20/26
framework) whose thesis is *high-level, JavaScript-like ergonomics for
prototyping without sacrificing speed*. Hydrargyros is **not** a gold branch — it
lives in its own repo and is free to diverge. Gold stays untouched.

The name is a deliberate joke and a real signal: gold is **Au**, Hydrargyros is
**Hg** — both are chemical-element metals, so the pairing keeps the parallel.
The planet and the swift messenger god, fitting the "fast prototyping" goal.

## Core thesis

gold's unique idea is adding accessible, JS-like syntax on top of a fast core.
Hydrargyros carries that forward but is honest about where Rust can and can't
copy JavaScript:

- **What ports faithfully:** the `var` value, prototype-based objects,
  ordered lists, the `o!`/`a!` literal macros, template strings, and the
  array helpers.
- **What can't port (and the Rust-native replacement):**
  - gold's implicit `v["k"] = 5` assignment-conversion → explicit
    `.get()` / `.put()` / `.set()`. This trades a footgun for borrow-checked
    safety — the chaos gold's implicit casts caused is unrepresentable here.
  - gold's `varRef::operator=` → the `VarRef` proxy's `.put()`.
- **What's better in Rust:** explicit typed reads via `Into<…>` / `as_*()`,
  a `Null` literal instead of a verbose type tag, and (planned) `#[derive]`
  traits and serde — the type system does work gold had to paper over.

## Current state — Core module

The core is implemented and tested (`tests/ergonomics.rs`, `tests/serde.rs`,
`tests/func.rs`, `tests/threads.rs`; the web layer in `tests/web.rs`,
`tests/web_server.rs`, `tests/html.rs`, `tests/css.rs`, `tests/store.rs`):

| File | What |
| --- | --- |
| `src/var.rs` | `Var` — the COW dynamic value; `Ty` tag; arithmetic; `From` conversions; `Null` literal; `Func` (id-bearing function value); serde `Serialize`/`Deserialize` |
| `src/object.rs` | `Object` — insertion-ordered map + prototype chain (`set_parent`/`inherits`); serde |
| `src/list.rs` | `List` — ordered array, grows-with-nulls on index-set |
| `src/proxy.rs` | `VarRef` — write-back proxy (`v.at("k").put(5)`), mutates the shared value in place |
| `src/sugar.rs` | `o!{}`, `a![]`, `tpl!`, `func!`, `each`/`map`/`filter`/`find`/`join` |
| `src/web/html.rs` | HTML5 builder + parser — elements as data (`tag`/`attr`/`items`), escaping, void tags, entity decoding, implicit closes, malformed-input recovery |
| `src/web/css.rs` | CSS parser — rules + typed declarations, `!important` as `name!` flags, at-rule skipping |
| `src/web/store.rs` | file-backed document store — collections, `$set`/`$unset`/`$inc`, `$in` queries, `_id` generation |
| `src/web/database.rs` | the `database` facade — config in, collection handles out |
| `src/web/http/` | http layer (behind the `web` feature) — `Server` facade (verbs, `:param`/`*` patterns, `ws`, `mount`, `on_error`, `start`/`stop`), `Response`, request staging; `post_var`/`get_var` sugar; axum underneath |
| `src/from_var.rs` + `derive/` | `FromVar` — the typed hot-path seam: `#[derive(FromVar)]` turns a `Var` object into a strict, path-tracked struct |
| `src/game/` | game module — `Engine` (gold's pump), `Component` (priority tiers), `Entity`/`Transform` (scene tree), `WindowSystem` registry + `HeadlessWindow`, physics (`PhysicsWorldBackend` + `none`/rapier backends, `PhysicsBody` step-and-sync), `RenderBackend` + offscreen wgpu renderer, `ScriptedComponent` (the `script` feature's interpreter seam) |

Key design choices:

- **`Arc<Mutex<…>>` payloads.** This is what lets the `VarRef` proxy mutate
  the *original* shared value in place — the faithful gold behavior — and it
  makes the whole value graph `Send + Sync`: the same `Var` crosses threads,
  async tasks, and web handlers (pinned by a compile-time assertion in
  `lib.rs`). Locks are poison-tolerant (`RefCell` never poisoned either).
- **serde is the non-negotiable serialization layer.** One `Serialize`/
  `Deserialize` impl on `Var` replaces gold's hand-rolled JSON/BSON/CBOR/
  MsgPack/UBJSON — any serde format works for free. The `Func` variant has no
  data-model form, so it serializes as `null` (you can't round-trip a closure);
  an id→Func registry could later round-trip handles as `{"__func__": id}`.
- **`o!`/`a!`** (renamed from `jo!`/`ja!`): `o` reads as "object" instantly;
  the `j` was only there to disambiguate for C++/HTML keywords.

## Where Hydrargyros goes from here

The plan follows gold's own module structure, adapted to Rust's ecosystem
rather than reinventing wheels.

### 1. Core (done) → harden

- wgpu-free today; the core is pure data.
- `FromVar` (done): the typed hot-path seam — `#[derive(FromVar)]` turns a
  `Var` object into a plain struct with strict, path-tracked reads
  (`"user.stats.score": expected int, found string`), `rename`/`default`
  field options, and `Option`/`Vec`/nested-struct support. Prototyping code
  passes `Var`; the hot loop works with real types. Lives in the
  `hydrargyros-derive` proc-macro crate, re-exported at `hydrargyros::FromVar`.
- Functions (done): `Func` carries a stable id, so `Var` equality compares
  functions by id — clones equal their original; fresh functions never compare
  equal. Still open: the id→Func registry for serde round-tripping.

### 2. Web module

gold had a web/UI layer; Hydrargyros's web layer leans on Rust's mature HTTP
ecosystem instead of reinventing it.

*Server facade (gold parity) + the sibling parsers/store:* `hydrargyros::web`
mirrors gold's `web::server` — `get`/`post`/`put`/`patch`/`delete`/`options`
routes with `:param` and trailing-`*` patterns (gold's uWS conventions),
requests staged as `Var` (`method`/`path`/`params`/`query`/`headers` + a body
parsed by content type), `ws` routes with `open`/`message`/`close` and a
`send`/`close` socket handle, static-file `mount`s with mime detection, an
`on_error` hook for the 404 page, and graceful, restartable `start()`/`stop()`.
Tests drive real HTTP and WebSocket wire traffic; `examples/web_site.rs` is a
multi-page homage to gold's `myWebProject`, built with the HTML builder.

The siblings are ported too, always available (pure data, no feature gate):
`web::html` — the HTML5 builder + parser (gold's `iHTML`/`Parser`, including
void tags, entity decoding, implicit closes, and malformed-input recovery);
`web::css` — rules with typed declarations and `!important` flags;
`web::store` — the file-backed document store with `$set`/`$unset`/`$inc`,
`$in` queries, and Mongo-shaped 24-char ids; `web::database` — gold's
database facade (config in, collection handles out). Tests are ported from
gold's `webTest.cpp`.

Streaming and TLS complete gold's server surface: `Response::stream` (gold's
`write`/`end` loop — chunked out, the SSE shape), `route_stream` (gold's
`onData` — async handlers over the raw request-body stream), and the
`sslCert`/`sslKey` config pair switching the vhost to HTTPS (axum-server's
rustls transport, verified by a real handshake against a self-signed cert).
Stop is graceful with a bounded grace period and servers restart in one
process. Remaining divergences, on purpose: gold's uWS streaming internals
(`cork`/`onWritable`/`writeContinue`) don't map to hyper, and the `database`
facade ships only the `"file"` backend, like gold.

- **`hyper`** (or `axum`, which is built on hyper) for the HTTP server — the
  wheel for request/response handling.
- **`socket2`** underneath where we need low-level socket control that hyper
  doesn't expose.
- **`tokio` + `futures`** for async. Hydrargyros's data flows through `Var`, so a
  request handler is just a function `Var -> Var` (input params → response),
  which composes beautifully with async: `async fn handler(req: Var) -> Var`.
- The goal: an expressjs-like surface where routes are JS-style closures over
  `Var`, not typed request/response structs.

### 3. Game module

Ported from gold's `game` module, headless first (gold ships a
`windowSystemHeadless` too — the whole loop runs and tests without a GPU):

- **Engine** — the pump + dispatch loop, gold's exact frame: poll window
  events (`quit` stops the loop after the in-flight frame), sort + init
  components, priority-ordered `update(dt)`, the draw pass, then the
  frame-rate cap. `update()`/`draw()` are public so headless code and tests
  drive frames without a window. Settings ride the same JSON seam.
- **Component** — gold's priority tiers (high/data/physics/generic/draw,
  custom values in user space) with `initialize`/`update`/`draw` hooks, as
  a Rust trait; typed lookup by downcasting.
- **Entity/Transform** — the scene tree: transforms (position/rotation/
  scale) composing parent-first down the hierarchy, enable/disable.
- **Windowing** — gold's registered-backend pattern as Rust traits and
  register/create registries; `headless` is pre-registered, events cross the
  boundary as `Var` (`o!{ "type": "quit" }`). The **streaming window**
  (behind the `stream` feature) turns the headless render buffer into a
  browser page: a canvas app auto-opens on launch (`--no-open` to skip —
  paste the URL anywhere, e.g. an embedded browser), frames stream over
  WebSocket (`[w][h][rgba]` binary), and browser keys/mouse/wheel come back
  through `WindowSystem::poll` as `Var` events — the engine can't tell it
  apart from SDL, and the whole loop is inspectable by an AI or a test. The
  **winit backend** (behind the `winit` feature) is the classic desktop
  window: the event loop runs on the main thread, the pump on another
  thread through the same seam, and the renderer presents to the surface
  directly (resize-aware).
- **Physics** (behind the `physics` feature) — gold's `world`/`physicsBackend`
  split: a `PhysicsWorldBackend` trait + registry, the built-in no-op `none`
  backend (bodies no-op, transforms stay under script control — gold ships
  the same fallback), and **rapier3d** as the real simulation: box/sphere
  rigid bodies, gravity, stepping, raycasts that name the hit body. The
  engine materializes `PhysicsBody` components at their entity transforms
  and syncs poses back every frame (gold's step-and-sync). The pump takes a
  fixed timestep for deterministic headless sims.
- **Render** (behind the `wgpu` feature) — a `RenderBackend` trait and an
  **offscreen wgpu renderer**: no window, no surface — frames render to a
  texture, get read back, and can be saved as PNGs (gold's screenshot debug
  aid), so tests assert actual pixels. The scene is camera-driven: a
  `Camera` (position + half-extents, world → clip; z is the depth window)
  and `MeshRenderer` components — entities render themselves through the
  draw pass at their composed world transforms. Two pipelines share the
  frame: the quad scene (rect/circle silhouettes, SDF-clipped, list order)
  and the **mesh scene** — models (procedural cube/sphere/quad, or glTF
  files via the `gltf` feature: node transforms and material base colors
  baked into vertices, one part per base-color texture) drawn depth-tested
  with back-face culling, model matrices built from the entity transform
  (`T·R·S`). Textures sample at the vertex UVs (glTF's top-left uv origin)
  with a 1×1-white fallback for untextured parts; `MeshRenderer::textured`
  over `Mesh::quad` is the sprite path (images decode via the `image`
  feature). **Lights** are gold's `light` component —
  `point`/`directional`/`spot` at entity transforms (position = world
  origin, direction = world forward), gold's prototype defaults
  (white point at intensity 1, cone `(1, 1)`), packed per light in gold's
  four uniform rows (`dir+range / color+intensity / pos+innerCos /
  outerCos+type`) and shaded per-fragment with the KHR punctual math gold's
  PBR shader ships (range attenuation `(1 − (d/r)⁴)⁺/d²`, smoothstep spot
  cones) over a fixed ambient floor. The draw pass gathers light
  components into the scene's light list; a scene with none renders
  ambient-only (gold's zeroed light contributes nothing). Gold shipped
  `LIGHT_COUNT=1`; this port spends the bank on eight.

Next: camera 3D — perspective/look-at projection with orbit controls, so the
mesh scene renders a real third-person view.

### Scripting layer (cross-cutting) — done (v1, the interpreter seam)

- **The crate is v12** (DevYatsu's `v12`, git-only at a pinned rev — the
  roadmap's "DevYatsu's script crate" name was wrong; the dependency is
  `v12-api`, the engine's embedding facade). DevYatsu also made an
  expressjs-like web lib, so it pairs with the web module. Hydrargyros's
  `Var` is the bridge between native Rust and scripts, mirroring gold's
  `script` facade (`include/lang/lang.hpp`): `Script::new({"source"})`,
  `load`, `run`, `set_global`/`get_global`, `call`, `register` — behind the
  `script` feature.
- **A Send + Sync handle over a !Send engine.** v12's `Context` is
  single-mutator (!Send + !Sync), while `Var` is `Send + Sync` and
  `Component: Send` — and the winit pump runs on its own thread. So
  `Script` is a lightweight handle (id + source + Send host closures) and
  the `Context` lives in a **thread-local pool keyed by script id**,
  materialized on first use per thread: all calls for one script happen on
  one thread and state persists per (script, thread). Moving a script to
  another thread rematerializes it there — the pool design is pinned by a
  test that moves a handle into `thread::spawn`.
- **The JSON bridge (v1).** v12's facade marshals only scalars, its
  completion values are unwired (their ADR-004), and objects have no host
  API — so values cross as JSON text over host-function calls: `serde_json`
  on the `Var` side, `JSON.parse`/`JSON.stringify` on the script side, with
  explicit fidelity checks (integers beyond 2^53 and non-finite floats are
  errors, not silent degradations; a script function value crossing out is
  an error too). The per-frame path stays scalar: `call_unit` drives hook
  shims with a bare `f64` — no JSON, no recompile.
- **`ScriptedComponent`** (gold's optional lifecycle): `initialize`/
  `update(dt)`/`draw` dispatch to same-named script functions, missing ones
  silently skipped, script throws fail loud. The transform seam: the
  component carries the entity transform as a shared
  `Arc<Mutex<Transform>>` handle the script sees as a `transform` global —
  refreshed before each hook, written back after (the engine materializes
  `PhysicsBody` poses itself; script writes go through the component's
  handle, and an engine-side link like physics' is the natural next step).
- Honest divergences: v12 compiles-and-runs in one step (gold splits
  load/run), each eval's top-level `let`/`const` is lexical to that eval
  (global-object state is what `get_global` reads), and object key order is
  preserved through the bridge only because serialization streams the `Var`
  directly. A `Var::Func` crossing in becomes a script-callable host closure;
  the serde `{"__func__": id}` contract lets function handles ride data.
- The ambition stands: write a prototype in `Var`/script ergonomics, then
  derive a typed native path for the hot loop. That's gold's thesis, made
  Rust-native.
- Portability wart, upstream's: the pinned v12 tree carries a
  `conformance/test262` gitlink with no `.gitmodules` mapping, so `cargo
  fetch` of the dependency fails on a machine without a pre-seeded cargo git
  checkout (this machine's is seeded — the submodule-update error is the
  tell). Builds resolve fine once checked out; upstream dropping the gitlink
  or adding the module mapping fixes it properly.

## What Hydrargyros will NOT do

- **Not integrate into Bevy.** Bevy is ECS-first (homogeneous typed components,
  archetype storage); `Var` is type-erased and shared-mutable. Forcing `Var`
  into ECS systems fights the borrow checker and throws away both systems'
  strengths. Hydrargyros is the scripting/data layer; others may *choose* to bridge
  it to Bevy later, but Hydrargyros doesn't depend on or bend for Bevy.
- **Not reinvent wheels.** serde (serialization), hyper (HTTP), wgpu (graphics),
  tokio/futures (async), DevYatsu's v12 (the interpreter) — each is the right
  existing tool. Hydrargyros's value is the `Var` seam and the ergonomics, not
  reimplementing the Rust ecosystem.

## Verify

```
cargo test                  # core tests
cargo test --all-features   # + web layer
cargo clippy --all-targets
```
