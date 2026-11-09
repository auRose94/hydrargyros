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
  `hyg-derive` proc-macro crate, re-exported at `hyg::FromVar`.
- Functions (done): `Func` carries a stable id, so `Var` equality compares
  functions by id — clones equal their original; fresh functions never compare
  equal. Still open: the id→Func registry for serde round-tripping.

### 2. Web module

gold had a web/UI layer; Hydrargyros's web layer leans on Rust's mature HTTP
ecosystem instead of reinventing it.

*Server facade (gold parity) + the sibling parsers/store:* `hyg::web`
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
  apart from SDL, and the whole loop is inspectable by an AI or a test.
  The surface is rotation-aware and hand-off capable: fit mode upscales
  bilinearly (`image-rendering: auto` — `pixelated` would crunch every
  non-integer scale) with letterbox math recomputed on
  `orientationchange`/viewport resizes, the page is phone-sized (viewport
  meta with `viewport-fit=cover`, `touch-action: none` on the canvas,
  44px-ish touch targets), and a ☿ toolbox menu carries the controls hints
  plus the LAN-reachable URL (the UDP-connect trick names the interface IP;
  a loopback bind keeps the QR harmless) for opening the stream on another
  device — the page's centered symbol **is** that QR itself: the `/qr` SVG
  (qrcodegen at ECC High) inlines into the template with the tray mark
  riding a white plate in its center (`QR_LOGO_PLATE_FRACTION`, ~13% of the
  code area — decode-verified back to its URL by the rqrr test in
  tests/stream.rs), and hovering/tapping it shows the hand-off info. The
  fullscreen button hands desktop browsers off to the **native view**
  (`native_view::NativeView`, behind `winit`+`wgpu`) — a
  borderless-fullscreen winit window whose mini blit renderer consumes the
  SAME `FrameSink`, a second consumer of the buffer; the pump never
  restarts, Esc closes the view only (it has no path back into the
  engine's event queue), and frames skip by generation so an unchanged
  buffer is never re-presented. Phones instead do device fullscreen
  (requestFullscreen + a best-effort orientation lock). An opt-in
  `.with_token()` gates the page and the WS upgrade behind a random hex
  `?t=` token (stdlib entropy, off by default — zero behavior change
  without it). The
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
  fixed timestep for deterministic headless sims. `set_gravity` is a LIVE
  knob — it wakes every body, because settled bodies sleep and a gravity
  flip would otherwise silently no-op on any resting scene (the sandbox
  demo's antigravity frame was identical to the settled one until this; a
  divergence from gold, whose bullet backend only sets gravity at world
  init). Bodies carry gold's
  `mass` proto knob: an explicit mass makes a dynamic body weigh exactly
  that (rapier derives the shape's inertia at the given mass — gold's
  `calculateLocalInertia`), mass 0 keeps the density-derived default, and
  mass on a static body is ignored — a deliberate divergence, since gold
  collapses static/dynamic into `mass != 0` while the port keeps
  `BodyType` explicit (we have both knobs). That closes the
  physics-leftovers question honestly: gold's bullet backend never
  constructs a constraint — no joint/constraint/force/impulse/kinematic/
  collision-event surface exists anywhere in gold's src, include, or tests
  — so there is nothing to port; porting it would be invention, not
  porting. If a future port needs joints, rapier's `ImpulseJointSet` is
  the designated slot. The one real gold surface deliberately not ported
  is `debugDraw`/wireframe — render-coupled, and untested even in gold.
- **Render** (behind the `wgpu` feature) — a `RenderBackend` trait and an
  **offscreen wgpu renderer**: no window, no surface — frames render to a
  texture, get read back, and can be saved as PNGs (gold's screenshot debug
  aid), so tests assert actual pixels. The scene is camera-driven: a
  `Camera` (eye/center position + look-at target + projection, world →
  clip) and `MeshRenderer` components — entities render themselves through
  the draw pass at their composed world transforms. Two pipelines share the
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

**Camera 3D** completes the scene: the `Camera` carries a projection — the
v0 orthographic window (position + half-extents, z the mesh depth window)
or gold's perspective (fov 60° y / near 0.1 / far 100, gold's prototype
defaults; aspect = width/height — gold's `ratio`, set by the embedder;
resize-aware aspect is a later refinement). The perspective matrix is
gold's camera math (camera.cpp `setViewTransform`): bx's `mtxLookAt` +
`mtxProj(…, homo=false)` — z ∈ [0, 1] after the divide, exactly WebGPU's
clip z ∈ [0, w] convention, so `CompareFunction::Less` keeps meaning
"closer wins" (the GL-style [−1, 1] mapping would clip the whole near
half). One divergence, on purpose: gold's look-at is left-handed (bx's
default, view-space +z toward the target), but the mesh pipeline's
back-face winding was established by the v0 ortho camera (GL-style sight
−z), so the perspective view uses bx's right-handed variant and ONE cull
mode stays correct for both cameras — the left-handed one renders meshes
inside-out (far faces pass cull instead of near ones; the pixel tests
catch it). The quad scene stays an orthographic image of the world — a
second divergence from gold, which projects everything through the one
camera: under a perspective camera quads map through the ortho window
derived at the target plane (`quad_window`: half_y = distance·tan(fov/2),
half_x over the aspect), so flat scene furniture lines up at the target's
depth. The `OrbitController` is Hydrargyros-native (gold has none — its
camera rides the script seam): yaw/pitch/distance around a target, fed
the unified mouse/wheel events (left-drag orbits — button 0 is left in
both winit and the DOM, which diverge on right/middle; wheel zooms,
negative delta zooms in), building the perspective camera each frame;
`examples/game_gltf.rs` streams the checker cube through it as a real
third-person view, and `examples/game_sandbox.rs` combines every rung —
rapier physics (gravity flips on space, `r` resets the stack), a
v12-scripted patroller moving its entity through the engine-side link,
lights, procedural meshes — into one orbit-driven sandbox streamed to the
browser.

**App services** — two app-development fills beside gold's own surface:

- **Timers/tweening** (`game::tween`, feature-free) — Hydrargyros-native
  (gold has no tween/timer utility; its only easing is the UI style
  system's CSS timing functions, style.cpp `easeValue`): an `Easing` enum
  (linear/quad/cubic/sine/elastic curves, `ElasticOut` overshooting then
  settling exactly onto 1), a `Tween` (`step(dt)` returns the current eased
  value, clamped at the end so the last frame lands on `to`; durations
  clamp to a `MIN_DURATION`, the OrbitController clamp pattern), and a
  `Timer` (`step(dt)` true on tick — repeating timers re-arm carrying the
  leftover, so ticks are a function of accumulated time only and frame
  jitter can't drift the schedule; one-shots fire once). Pure data, no
  engine wiring — v1 is a utility, components compose it in `update(dt)`.
- **Audio** (`game::audio`, behind the `audio` feature) — gold DOES have an
  `audioSystem` here (backends registered by name,
  `loadSound(path)`→handle, `play(handle, loop)`, `stop()` everything,
  `setVolume` master [0, 1] — SDL adapters as plugins), so the shape
  mirrors it: an `AudioBackend` trait + register/create registry (gold's
  factory map under a mutex), the real device being **rodio** (cpal — the
  wheel gold loads SDL for), and the headless-first `none` recorder backend
  (gold's registry has NO fallback — `createAudioSystem` returns nullptr on
  a miss; the port ships one, the physics pattern, its play records
  introspectable through `AudioWorld::none_plays` so contracts test without
  a device). Divergences, on purpose: `play` takes the source directly —
  gold's load-then-play two-step collapsed — and returns a per-play id, so
  `stop(id)` names ONE sound where gold's `stop()` stops everything;
  `AudioSource::Tone` synthesizes PCM in-memory (44.1 kHz mono i16, a
  ~20-line stdlib WAV encoder) and the test decodes those bytes with
  rodio's real Decoder, no device involved. The engine carries the world
  like its physics world (`set_audio`/`audio_mut`).

Next: the rung queue is complete — new work branches from the roadmap
sections above.

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
  refreshed before each hook, written back after, and the engine closes the
  loop (`Engine::script_sync`, the physics sync's sibling): an unseeded
  component spawns at its entity's transform (gold's physicsBody rule), and
  after every component pass each handle writes back into its entity's
  transform, so the draw pass sees the scripted pose. One mover per entity:
  physics XOR script — an entity with both syncs in undefined order.
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
