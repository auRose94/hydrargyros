# hyg ☿ roadmap

hyg (Hydrargyros) is a standalone Rust framework whose thesis is *high-level,
JavaScript-like ergonomics for prototyping without sacrificing speed*. It is
its own project, free to diverge from any ancestor, and its docs and code are
standalone-first: nothing here requires knowing where the design came from.

The name is a signal: *hydrargyrum* — Greek, "water-silver" — the element
mercury, **Hg**, the planet and the swift messenger god, fitting the "fast
prototyping" goal. The crate/namespace is `hyg`.

> **Heritage.** hyg's design descends from an earlier C++ framework. The
> record of what was mirrored, what diverged on purpose, and why lives in
> [gold-lineage.md](gold-lineage.md) — archaeology, not prerequisite. This
> roadmap and the code assume no knowledge of it.

## Core thesis

hyg's unique idea is accessible, JS-like ergonomics on top of a fast core —
honest about where Rust can and can't copy JavaScript:

- **What ports faithfully:** the `var` value, prototype-based objects,
  ordered lists, the `o!`/`a!` literal macros, template strings, and the
  array helpers.
- **What hyg refuses (and the Rust-native replacement):** implicit
  `v["k"] = 5` assignment-conversion is a footgun — explicit
  `.get()` / `.put()` / `.set()` replaces it, and the chaos implicit casts
  allow is unrepresentable under borrow checking. The `VarRef` proxy's
  `.put()` carries the write-through-a-handle case.
- **What's better in Rust:** explicit typed reads via `Into<…>` / `as_*()`,
  a `Null` literal instead of a verbose type tag, `#[derive(FromVar)]` and
  serde — the type system does work that dynamic languages paper over.

## Current state

The core and both application modules are implemented and tested. Tests are
integration-style in `tests/`, one file per area, and drive real behavior:
real HTTP and WebSocket traffic, real TLS handshakes, real pixels.

| File | What |
| --- | --- |
| `src/var.rs` | `Var` — the COW dynamic value; `Ty` tag; arithmetic; `From` conversions; `Null` literal; `Func` (id-bearing function value, `{"__func__": id}` serde handles); serde `Serialize`/`Deserialize` |
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
| `src/script.rs` | the `script` feature's interpreter seam — v12 behind a `Send + Sync` handle, JSON bridge, `ScriptedComponent` lifecycle |
| `src/game/` | game module — `Engine` (the pump), `Component` (priority tiers), `Entity`/`Transform` (scene tree), window backend registry (headless/winit/streaming/native/tray), physics (`PhysicsWorldBackend` + `none`/rapier backends), offscreen wgpu renderer, cameras, lights, `game::tween`, audio on rodio |

Key design choices:

- **`Arc<Mutex<…>>` payloads.** This is what lets the `VarRef` proxy mutate
  the *original* shared value in place, and it makes the whole value graph
  `Send + Sync`: the same `Var` crosses threads, async tasks, and web
  handlers (pinned by a compile-time assertion in `lib.rs`). Locks are
  poison-tolerant (`RefCell` never poisoned either).
- **serde is the non-negotiable serialization layer.** One
  `Serialize`/`Deserialize` impl on `Var` replaces a tower of hand-rolled
  format codecs — any serde format works for free. `Func` has no data-model
  form (closures can't cross processes), so it serializes as the handle
  `{"__func__": id}` and round-trips through the process-global
  `FUNC_REGISTRY`; unknown ids degrade to `Null`.
- **`o!`/`a!`**: `o` reads as "object" instantly; a `j` prefix would only
  disambiguate for C++/HTML keywords.

## 1. Core — done → harden

- wgpu-free; the core is pure data.
- `FromVar` (done): the typed hot-path seam — `#[derive(FromVar)]` turns a
  `Var` object into a plain struct with strict, path-tracked reads
  (`"user.stats.score": expected int, found string`), `rename`/`default`
  field options, and `Option`/`Vec`/nested-struct support. Prototyping code
  passes `Var`; the hot loop works with real types. Lives in the
  `hyg-derive` proc-macro crate, re-exported at `hyg::FromVar`.
- Functions (done): `Func` carries a stable id, so `Var` equality compares
  functions by id — clones equal their original; fresh functions never
  compare equal. The id→`FUNC_REGISTRY` serde round-trip is done
  (`{"__func__": id}` handles; unknown ids restore as `Null`).

## 2. Web module — done

An expressjs-like surface where routes are JS-style closures over `Var`, not
typed request/response structs, on Rust's mature HTTP ecosystem:

- **axum** (on hyper) for request/response handling; **tokio + futures** for
  async. A request handler is just `Var -> Var` (input params → response),
  which composes with async: `async fn handler(req: Var) -> Var`.
- **socket2** underneath where low-level socket control matters later.

*Server facade + the sibling parsers/store:* `hyg::web` gives `get`/`post`/
`put`/`patch`/`delete`/`options` routes with `:param` and trailing-`*`
patterns (uWS conventions), requests staged as `Var`
(`method`/`path`/`params`/`query`/`headers` + a body parsed by content
type), `ws` routes with `open`/`message`/`close` and a `send`/`close` socket
handle, static-file `mount`s with mime detection, an `on_error` hook for the
404 page, and graceful, restartable `start()`/`stop()`. Tests drive real
HTTP and WebSocket wire traffic; `examples/web_site.rs` is a multi-page demo
built with the HTML builder.

The siblings are always available (pure data, no feature gate): `web::html`
— the HTML5 builder + parser (void tags, entity decoding, implicit closes,
malformed-input recovery); `web::css` — rules with typed declarations and
`!important` flags; `web::store` — the file-backed document store with
`$set`/`$unset`/`$inc`, `$in` queries, and Mongo-shaped 24-char ids;
`web::database` — the facade (config in, collection handles out).

Streaming and TLS complete the server surface: `Response::stream` (chunked
out, the SSE shape), `route_stream` (async handlers over the raw
request-body stream), and the `sslCert`/`sslKey` config pair switching the
vhost to HTTPS (axum-server's rustls transport, verified by a real
handshake against a self-signed cert). Stop is graceful with a bounded
grace period and servers restart in one process — the watch-based design
with a polled grace drain (axum-server's notify can be missed between
accept-loop iterations; if shutdown hangs reappear, suspect lost-notify
first). Divergences, on purpose: uWS streaming internals
(`cork`/`onWritable`/`writeContinue`) don't map to hyper, and the
`database` facade ships only the `"file"` backend.

## 3. Game module — done, headless first

The whole engine loop runs and tests without a GPU or window:

- **Engine** — the pump + dispatch loop, the exact frame: poll window events
  (`quit` stops the loop after the in-flight frame), sort + init components,
  priority-ordered `update(dt)`, the draw pass, then the frame-rate cap.
  `update()`/`draw()` are public so headless code and tests drive frames
  without a window. Settings ride the JSON seam.
- **Component** — priority tiers (high/data/physics/generic/draw, custom
  values in user space) with `initialize`/`update`/`draw` hooks, as a Rust
  trait; typed lookup by downcasting.
- **Entity/Transform** — the scene tree: transforms (position/rotation/
  scale) composing parent-first down the hierarchy, enable/disable.
- **Windowing** — the registered-backend pattern as Rust traits and
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
  without it). The **winit backend** (behind the `winit` feature) is the
  classic desktop window: the event loop runs on the main thread, the pump
  on another thread through the same seam, and the renderer presents to
  the surface directly (resize-aware).
- **Physics** (behind the `physics` feature) — the `PhysicsWorldBackend`
  trait + registry, the built-in no-op `none` backend (bodies no-op,
  transforms stay under script control), and **rapier3d** as the real
  simulation: box/sphere rigid bodies, gravity, stepping, raycasts that
  name the hit body. The engine materializes `PhysicsBody` components at
  their entity transforms and syncs poses back every frame (step-and-sync).
  The pump takes a fixed timestep for deterministic headless sims.
  `set_gravity` is a LIVE knob — it wakes every body, because settled
  bodies sleep and a gravity flip would otherwise silently no-op on any
  resting scene (the sandbox demo's antigravity frame was identical to the
  settled one until this). Bodies carry the `mass` proto knob: an explicit
  mass makes a dynamic body weigh exactly that (rapier derives the shape's
  inertia at the given mass), mass 0 keeps the density-derived default,
  and mass on a static body is ignored — `BodyType` keeps static/dynamic
  explicit. The joints/constraints question is closed honestly: the design
  ancestors never constructed a constraint — no joint/constraint/force/
  impulse/kinematic/collision-event surface exists in their sources or
  tests — so there is nothing to port; if a future need arises, rapier's
  `ImpulseJointSet` is the designated slot. Wireframe `debugDraw` was never
  shipped (render-coupled and untested upstream).
- **Render** (behind the `wgpu` feature) — a `RenderBackend` trait and an
  **offscreen wgpu renderer**: no window, no surface — frames render to a
  texture, get read back, and can be saved as PNGs (the screenshot debug
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
  feature). **Lights** — `point`/`directional`/`spot` at entity transforms
  (position = world origin, direction = world forward), prototype defaults
  (white point at intensity 1, cone `(1, 1)`), packed per light in four
  uniform rows (`dir+range / color+intensity / pos+innerCos /
  outerCos+type`) and shaded per-fragment with the KHR punctual math
  (range attenuation `(1 − (d/r)⁴)⁺/d²`, smoothstep spot cones) over a
  fixed ambient floor. The draw pass gathers light components into the
  scene's light list; a scene with none renders ambient-only. The bank
  spends eight lights.

**Camera 3D** completes the scene: the `Camera` carries a projection — the
v0 orthographic window (position + half-extents, z the mesh depth window)
or the perspective projection (fov 60° y / near 0.1 / far 100 defaults;
aspect = width/height, set by the embedder; resize-aware aspect is a later
refinement). The perspective matrix is look-at + projection mapping the
depth window to z ∈ [0, 1] after the divide — exactly WebGPU's clip
z ∈ [0, w] convention, so `CompareFunction::Less` keeps meaning
"closer wins" (the GL-style [−1, 1] mapping would clip the whole near
half; this trap bit once already). One deliberate choice: the look-at uses
the right-handed variant (view-space −z forward) so ONE back-face cull mode
stays correct for both cameras — a left-handed view renders meshes
inside-out (far faces pass cull instead of near ones; the pixel tests
catch it). The quad scene stays an orthographic image of the world — under
a perspective camera quads map through the ortho window derived at the
target plane (`quad_window`: half_y = distance·tan(fov/2), half_x over the
aspect), so flat scene furniture lines up at the target's depth. The
`OrbitController` is hyg-native: yaw/pitch/distance around a target, fed
the unified mouse/wheel events (left-drag orbits — button 0 is left in
both winit and the DOM, which diverge on right/middle; wheel zooms,
negative delta zooms in), building the perspective camera each frame;
`examples/game_gltf.rs` streams the checker cube through it as a real
third-person view, and `examples/game_sandbox.rs` combines every rung —
rapier physics (gravity flips on space, `r` resets the stack), a
v12-scripted patroller moving its entity through the engine-side link,
lights, procedural meshes — into one orbit-driven sandbox streamed to the
browser.

**App services** — two app-development fills:

- **Timers/tweening** (`game::tween`, feature-free) — hyg-native: an
  `Easing` enum (linear/quad/cubic/sine/elastic curves, `ElasticOut`
  overshooting then settling exactly onto 1), a `Tween` (`step(dt)` returns
  the current eased value, clamped at the end so the last frame lands on
  `to`; durations clamp to a `MIN_DURATION`), and a `Timer` (`step(dt)`
  true on tick — repeating timers re-arm carrying the leftover, so ticks
  are a function of accumulated time only and frame jitter can't drift the
  schedule; one-shots fire once). Pure data, no engine wiring — v1 is a
  utility, components compose it in `update(dt)`.
- **Audio** (`game::audio`, behind the `audio` feature) — an
  `AudioBackend` trait + register/create registry, the real device being
  **rodio** (cpal), and the headless-first `none` recorder backend (the
  registry always has a fallback; its play records are introspectable
  through `AudioWorld::none_plays`, so contracts test without a device).
  `play` takes the source directly and returns a per-play id, so `stop(id)`
  names ONE sound; `AudioSource::Tone` synthesizes PCM in-memory (44.1 kHz
  mono i16, a ~20-line stdlib WAV encoder) and the test decodes those
  bytes with rodio's real Decoder, no device involved. The engine carries
  the world like its physics world (`set_audio`/`audio_mut`).

## Scripting layer (cross-cutting) — done (v1, the interpreter seam)

- **The interpreter is v12** (DevYatsu's `v12`, git-only at a pinned rev;
  the dependency is `v12-api`, the engine's embedding facade). hyg's `Var`
  is the bridge between native Rust and scripts: `Script::new({"source"})`,
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
- **`ScriptedComponent`**: `initialize`/`update(dt)`/`draw` dispatch to
  same-named script functions, missing ones silently skipped, script throws
  fail loud. The transform seam: the component carries the entity transform
  as a shared `Arc<Mutex<Transform>>` handle the script sees as a
  `transform` global — refreshed before each hook, written back after, and
  the engine closes the loop (`Engine::script_sync`, the physics sync's
  sibling): an unseeded component spawns at its entity's transform, and
  after every component pass each handle writes back into its entity's
  transform, so the draw pass sees the scripted pose. One mover per entity:
  physics XOR script — an entity with both syncs in undefined order.
- Honest divergences: v12 compiles-and-runs in one step (load/run split is
  not available), each eval's top-level `let`/`const` is lexical to that
  eval (global-object state is what `get_global` reads), and object key
  order is preserved through the bridge only because serialization streams
  the `Var` directly. A `Var::Func` crossing in becomes a script-callable
  host closure; the serde `{"__func__": id}` contract lets function handles
  ride data.
- The ambition stands: write a prototype in `Var`/script ergonomics, then
  derive a typed native path for the hot loop. That's the thesis, made
  Rust-native.
- Portability wart, upstream's: the pinned v12 tree carries a
  `conformance/test262` gitlink with no `.gitmodules` mapping, so `cargo
  fetch` of the dependency fails on a machine without a pre-seeded cargo
  git checkout (this machine's is seeded — the submodule-update error is
  the tell). Builds resolve fine once checked out; upstream dropping the
  gitlink or adding the module mapping fixes it properly.

## The next arc

The port rung queue is closed; new work branches from these items:

1. **Standalone identity** (done): crate renamed to `hyg`; the narrative is
   standalone-first (README, roadmap, code comments); the heritage record
   lives in [gold-lineage.md](gold-lineage.md).
2. **Editor designs** (drafted): **Hydrargyros Studio**
   ([game-editor-design.md](game-editor-design.md)) — a unified engine &
   editor for the game module (`hyg` is its short name); **Hydrargyros
   Press** ([web-editor-design.md](web-editor-design.md)) — a WYSIWYG
   website/API editor for the web module. The **`.hyg` carrier type** is
   established: a serialized `Var` whose terminal `.hyg` suffix is the file
   type and whose preceding suffix names the serde format — `.json.hyg`
   default, `.cbor.hyg`/`.bson.hyg` for dense scenes. Drafts for review,
   not commitments.
3. **Examples tour**: every feature area reachable from a runnable example,
   indexed in `examples/`.
4. **Test hardening** (done): the four long-broken doctests fixed — the
   suite is fully green in both modes; `List` (order, grows-with-nulls,
   self-extend snapshot, serde) and `Object::prepend` move-to-front
   contracts now pinned in tests.
5. **Cross-platform** (done — structurally): hardcoded `/tmp` paths moved
   to `env::temp_dir()`; the tray deps are target-split in Cargo.toml
   (Linux GTK + libappindicator, Windows/macOS native tray-icon). **Windows
   x86_64 is compile-verified** — `cargo check --all-features --target
   x86_64-pc-windows-gnu` passes under the mingw-w64-rust-bin toolchain.
   The support matrix and the rules for new code live in
   [platforms.md](platforms.md); Windows runtime + macOS verification
   remain open (real machines or CI runners).

## What hyg will NOT do

- **Not integrate into Bevy.** Bevy is ECS-first (homogeneous typed
  components, archetype storage); `Var` is type-erased and shared-mutable.
  Forcing `Var` into ECS systems fights the borrow checker and throws away
  both systems' strengths. hyg is the scripting/data layer; others may
  *choose* to bridge it to Bevy later, but hyg doesn't depend on or bend
  for Bevy.
- **Not reinvent wheels.** serde (serialization), hyper (HTTP), wgpu
  (graphics), tokio/futures (async), DevYatsu's v12 (the interpreter) —
  each is the right existing tool. hyg's value is the `Var` seam and the
  ergonomics, not reimplementing the Rust ecosystem.

## Verify

```
cargo test                  # core tests
cargo test --all-features   # full suite: web, physics, wgpu, stream, gltf, winit, tray
cargo clippy --all-targets --all-features
```

The suite is fully green — core and `--all-features`, doctests included.
