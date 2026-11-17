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
2. **Editor designs** (drafted; **M0 landed** — see item 7): **Hydrargyros
   Studio** ([game-editor-design.md](game-editor-design.md)) — a unified
   engine & editor for the game module (`hyg` is its short name);
   **Hydrargyros Press** ([web-editor-design.md](web-editor-design.md)) — a
   WYSIWYG website/API editor for the web module. The **`.hyg` carrier
   type** is established — now code, `hyg::carrier`: a serialized `Var`
   whose terminal `.hyg` suffix is the file type and whose preceding
   suffix names the serde format — `.json.hyg` default, `.cbor.hyg`/
   `.bson.hyg` designed dense upgrades. The Studio design
   also specs **canvas scaling** for non-standard ratios (Fit / Expand /
   Anchor with a derived 16:9 UI zone — ultrawide shows more world, not
   black bars, without re-anchoring the GUI) and the **Wayland-first,
   X11-second** windowing direction with **HDR** as the Wayland rung.
   Drafts for review, not commitments.
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
6. **The `dynamic` module** — **M0 landed (2026-11-09): the engine
   registry.** `hyg::dynamic::engine` is feature-free (the
   `ScriptEngine` factory / `ScriptSession` traits, the register/create
   registry with v12 pre-seeded, `ScriptError` as the shared error
   type); the v12 JSON bridge moved intact into `hyg::dynamic::v12`; and
   `Script` kept its exact public API — the script suite passes
   unchanged (22/22). The registry's own contracts are tested
   feature-free through a recording engine (the audio-`none` pattern).
   The new tests caught the module's first real bug on day one: a
   registry RwLock read-twice deadlock under parallel writer pressure.

6b. **The `dynamic` module — M1 landed (2026-11-14): FFI plugins, the
   trusted tier.** `hyg::dynamic::ffi` (behind the `dynamic-ffi` feature)
   loads native shared libraries as plugins: the mandated manifest export
   returns the `.json.hyg` Var document (kept on the plugin for
   introspection), the host checks the manifest's ABI version against its
   own, and every offered function registers into the **shared callable
   table** (`hyg::dynamic::functions`, feature-free — plugins, scripts,
   and graph function nodes are three faces of one table) as
   `"{plugin_id}.{fn}"` with pins mapped onto `Var`'s type vocabulary.
   **Hot swap** works the always-up way: a new version loads from a
   private side-by-side scratch copy (a rebuild that rewrites the source
   file never touches a mapped mapping), the plugin's namespace swaps in
   ONE atomic table write, in-flight calls drain against a bounded grace
   period, and the old library unloads exactly when the last in-flight
   call releases it (callers hold an `Arc` across the call — never
   unloaded under itself). A failed load or a different plugin's id swaps
   nothing; the serving version keeps serving. **Decision refined, not
   reversed:** abi_stable stays at the boundary for its layout-stable
   types (`RString`/`RVec`/`RBoxError`), but its `RootModule` loader was
   rejected after source verification — it caches the module in
   process-global statics (one load per process) and leaks the library by
   design, both incompatible with hot-swap. The mechanics are libloading
   (the same wheel abi_stable itself uses); the manifest ABI check plus
   the abi_stable data guarantee carry the safety role RootModule played.
   The contract lives in the **`hyg-plugin-api`** crate (workspace
   member): `DYNAMIC_ABI`, the manifest types, the mandated export
   signatures, and the three author rules (default system allocator,
   never unwind across the boundary, no `thread_local!` — each keeps a
   hot-unloadable library sound). The boundary is proven by an
   **out-of-tree test plugin** (`tests/ffi-plugin/`, excluded from the
   workspace — the third-party shape, depending on nothing but
   hyg-plugin-api + abi_stable + serde_json) built on demand by the
   tests: 5 boundary tests cover manifest/registration/calls, ABI-liar
   and corrupt-file refusals that swap nothing, in-place rebuild + reload
   (v1→v2 under the same process), the drain-grace timeout (the retired
   version's call completes correctly, new calls already hit v2), and the
   live-server swap — a slow HTTP request enters v1, the reload drains
   it, the request completes on v1, and the next request hits v2 with the
   pump never stopping.

6c. **The `dynamic` module — M2 landed (2026-11-14): the graph model.**
   `hyg::dynamic::graph::model` (feature-free, pure data): the `.hyg`
   graph-document schema — `graph` (name), `format`, `nodes` (`id`,
   `type`, editor `pos`, free-form `params`), `wires`
   (`from`/`from_pin`/`to`/`to_pin`), `entry` — with structural
   validation whose failures are `GraphError`s carrying **graph
   coordinates** (`node`, `pin`, message, and the `flow` chain the
   compilers will walk — empty for structural errors). The schema fixes
   the design sketch's wire shape: the sketch put `"pin"` on both ends
   of a wire, and duplicate JSON keys are unrepresentable — the landed
   document names each side (`from_pin`/`to_pin`). Validation is
   vocabulary-blind BY DESIGN: unique identifier node ids, wires and
   entries landing on known nodes, named identifier pins, the format
   gate (unknown `format` is an explicit error — migrations are the
   versioning path, never best-effort) — but fan-out is legal, cycles
   are legal (loops are real), and orphans/empty-entry sub-graphs are
   legal. Pin TYPES are the compilers' job (**type checking is pin
   checking**, at conversion — a graph is valid iff it converts, M3+).
   Round-trips: `Var` → `.json.hyg` text → `Var` is identity (insertion
   order survives Var's serde, verified against a real file);
   `to_var` regenerates the canonical document (schema fields, sketch
   order) — unknown extra fields are tolerated on read, dropped on
   regeneration. 9 tests in `tests/dynamic_graph.rs` (core suite):
   both round-trips, canonical regeneration, coordinate-carrying
   validation failures (unknown wire endpoint, unknown entry, duplicate
   id, non-identifier id, unnamed pins, format gate, path-voice shape
   failures), and the deliberate tolerances (fan-out, cycle, bare
   document, duplicate-wire refusal). Also: the M1 live-server test's
   entry window widened to a bounded 5s poll (the 1s window was flaky
   under full-suite parallel load). Next rung: M3 (graph → JS).
   **Shader graphs** — the material tier: same M2 model, a GPU-shaped
   second pin vocabulary, WGSL via naga (already in-tree via wgpu),
   dataflow cycles illegal — are now recorded in the design doc,
   deliberately outside the milestones until the event tier (M3–M6)
   lands; the graph side is nearly free, the renderer material system
   is the real arc.

6d. **The `dynamic` module — M3 landed (2026-11-14): graph → JS, the
   first compiler.** `hyg::dynamic::graph::to_js` (feature-free — the
   JS source is data until an engine runs it): a graph compiles to one
   `__hyg_handle(req)` function where **conversion IS validation** —
   kinds resolve, wires land on real pins of matching types, entry
   nodes are entry-capable, and every failure is a `GraphError` with
   graph coordinates; cycle failures walk the chain (the graph's stack
   trace: exec and data cycles both carry their own cycle as `flow`,
   read `a → b → a`). The M3 vocabulary: `web/request` (the staged
   request's params/query/headers/body as data outs), `web/respond`
   (a `return`; a path that never reaches one reads `Null` through the
   bridge — the undefined payload is already the protocol),
   `flow/branch` (then/else), pure expression nodes inlined at their
   use sites (`data/get`, `data/literal`, `math/compare`), and — the
   M1 sentence made load-bearing — **function nodes**: a node whose
   kind is a callable name (`js_route_greet`, `greeter.greet`) gets
   its pins FROM the callable's `FunctionMeta` and dispatches through
   a host binding into [`call_function`], the same table plugins
   registered into. Codegen is straight-line Blueprint semantics:
   non-pure data outs are function-scope lets (values produced in a
   branch flow out of it), pure nodes inline (a pure cycle is an
   expression cycle, caught at conversion), exec fan-out walks
   sequentially, converging paths re-execute the shared tail.
   Unwired pins read `undefined` (JS falsy rules; pin defaults are an
   editor concern until a vocabulary carries them). `route.rs`
   (script + web) materializes a compiled route as a `Script` — the
   `Send + Sync` handle over the thread-local session pool, one v12
   session per tokio worker — and serves it: 13 tests, including
   graph routes on a live server (`{"who":"Rose"}` → `"hello,
   Rose!"` through a function node, both sides of a branch driven by
   real request bodies, a failing callable a loud 500). One
   conversion-order voice decision: shape/type errors fail before
   anything executes, so their `flow` is empty like the model's
   structural errors; only cycles walk a chain (nothing was executing
   — or what executed IS the cycle). Next rungs: M4 (graph → Rust),
   then M5 (debugger + inspector), M6 (WASM tier).

6e. **The `dynamic` module — M4 landed (2026-11-14): graph → Rust, the
   native path.** **A compiled graph is a plugin.** The architecture
   refactor first: M3's conversion checker moved to
   `hyg::dynamic::graph::spec` — ONE validation pass (now including
   pure-node params) that both compilers share, so they can never
   disagree about what is valid; generators run only on a checked
   graph and trust it (emission is mechanical). `to_rust.rs` emits a
   **self-contained plugin crate** — the hyg-plugin-api contract's two
   mandated exports around a Var-native `fn route(req: &Var) ->
   Result<Var, String>` with NO JSON on the inside (marshaling once at
   the plugin boundary): `data/get` is `Var::get`, `data/literal`
   embeds its conversion-checked JSON, `math/compare` generates a
   numeric-then-string comparator yielding `Var::Bool`, respond is a
   `return Ok(...)` with `Ok(Var::Null)` fall-through (the JS path's
   undefined-reads-as-Null contract, native). **Function nodes link
   against project Rust** — the Blueprint→C++ story: `RustVocabulary`
   binds a callable name to a Rust path (convention
   `fn(&[Var]) -> Result<Var, String>`), the generated crate declares
   the dependency, and a binding whose signature disagrees with the
   function table's `FunctionMeta` is a conversion error (the JS and
   Rust compilers may never disagree about a graph). Node markers
   (`// [hyg:node:<id>]` before every node's code) + rustc span
   remapping: when the generated source fails to build — the design's
   "rare, only host-API drift" case, proven by a test binding a path
   that doesn't exist — the failure re-reports as a `GraphError`
   naming the node. The generated crate declares its own `[workspace]`
   (it may live inside the host project's tree), and `build_generated`
   shells the real toolchain: 6 tests in `tests/dynamic_graph_rust.rs`
   drive the whole pipeline — generate, `rustc`, `load_plugin` through
   the M1 boundary, the route in the function table (`rust_route.handle`),
   live HTTP serving `hello, Rose (native)!`, and the drift remap.
   Next rung: M5 (debugger + inspector), then M6 (WASM tier).

6f. **The `dynamic` module — M5 landed (2026-11-14): the debugger + the
   inspector.** **One implementation for both modules** — a web route's
   handler and a game frame's component are both "handlers over `Var`",
   and both debug through the same seam. The debugger is an
   **interpreter** (`hyg::dynamic::graph::debug`, feature-free): the
   compiled paths (JS, Rust) exist for speed; a debug execution walks
   the same validated graph, calls the SAME function table, and
   evaluates pure nodes with the canonical Var-native semantics
   (`native_compare` — the generated Rust helper and the JS loose
   semantics must agree, and the cross-path test proves the interpreter
   and the compiled JS route produce IDENTICAL results for the same
   graph and input). `DebugRun` is a state machine driven from two
   sides: the **executing side** (`drive` — one segment, to the next
   pause or the finish; non-blocking, so the game pump paces a graph
   *per frame* from a component's `update` — one node per frame, the
   pump IS the debugger's pacing; `run_blocking` parks a request's
   thread at every pause — a breakpoint freezes the frame) and the
   **debugger side** (`wait_event`, `watch` — any pin's live value,
   pure chains computing on demand, unexecuted outs an explicit error
   never a silent Null — plus `resume`/`step`/`abort` for stuck runs).
   Runtime errors carry the **actually walked path** as their flow
   chain — ground truth, not a prediction. Active runs register in a
   process-global registry — the seam an admin console or Studio panel
   attaches through. `GraphRoute::debug_handler` serves a route through
   the debugger: a real POST parks mid-flight at a breakpoint, the
   staged request visible at the pause, the response frozen until a
   client resumes — and then indistinguishable from the compiled path's
   response. The **inspector** (`hyg::dynamic::graph::inspect`,
   feature-free) renders every node's descriptor as a `.hyg` document —
   kind, summary, per-pin docs, doc links that resolve to real repo
   files (test-asserted), and function nodes surfacing the host
   function behind them (origin + summary from the table); unresolvable
   kinds are shown as unknown, not hidden. The descriptor refactor
   feeding it: pins became `PinSpec {name, ty, doc}`, kinds carry
   `summary`/`doc`. 9 tests in `tests/dynamic_graph_debug.rs`. Also:
   the M1 live-server test's entry window gained diagnostics on timeout
   (panicked task vs stalled dispatch) and M4's generated-crate builds
   moved to their own target dir — the intermittent 5s-window flake
   under full-suite load is retired with evidence. Next rung: M6 (the
   WASM tier). **Post-landing fix (2026-11-15): the live-route test's
   intermittent eternal hang, core-dump autopsied.** The web layer ran
   sync route handlers INLINE on runtime workers — and a handler that
   blocks (a debug run parked at a breakpoint) freezes the worker
   carrying tokio's driver: no timer and no IO event fires anywhere in
   the process, every async wait stalls forever, no panic, no output —
   the binary wedged ~1-in-5 hammer runs. `stage_req` now runs handlers
   on the **blocking pool** (`spawn_blocking`; a panicking handler is a
   loud 500, not a torn-down connection), the test matches its run by
   staged input (a process-global registry + parallel tests = a bare
   first-run grab steals foreign runs), an abort-on-drop `RunGuard`
   makes any failed assertion unwind the parked runner instead of
   stranding it, and `debug_handler` unregisters its run on unwind.
   The trap lives in AGENTS.md: handlers may block; workers may not.

6g. **The `dynamic` module — M6 landed (2026-11-14): the WASM tier, the
   milestone queue complete.** `hyg::dynamic::wasm` (behind
   `dynamic-wasm` — wasmtime 48 with cranelift+runtime+wat, no default
   features): a sandboxed plugin compiles and runs, and **permissions
   are enforced at the host boundary before anything registers**. The
   contract is the wasm mirror of the native plugin: exports `memory`,
   `alloc` (the guest's own bump allocator — the host writes ONLY into
   what alloc returned), `hyg_manifest` (the SAME `.json.hyg` Var
   document), and `hyg_call` (the dispatcher, JSON in/out — the same
   wire as the script bridge). The trust difference is the HOST, not
   the data: **no imports are bound** — no WASI, so no filesystem, no
   network, no clock; the module's whole world is its own memory and
   the bytes the host hands it. The manifest's requested permissions
   must be a subset of the host's capability grant
   (`load_wasm_plugin`'s `grants`) or the load refuses naming both
   sides; a broken guest allocator refuses loudly instead of writing
   into memory it does not own; missing mandated exports are explicit
   `Export` errors; an ABI-liar module refuses like its native
   sibling. Functions register into the SAME function table
   (`wasm:{id}:{version}` origins) — plugins of both tiers, scripts,
   and graph function nodes remain three faces of one table. The test
   plugin is authored as **WAT** (wasmtime parses text modules — the
   boundary proof needs no wasm32 toolchain; Rust→wasm32 via
   `rustup target add wasm32-unknown-unknown` is the authoring path
   for real plugins): 5 tests in `tests/wasm_plugin.rs` — load,
   manifest round trip, the FULL wire round trip (`echo` copies the
   args JSON verbatim through guest memory: ordered objects and nested
   shapes survive), permission refusal, ABI refusal, missing-export
   refusal, broken-allocator refusal. The milestone queue M0–M6 is
   complete: the engine registry, native FFI plugins, the graph model,
   two graph compilers (JS + Rust), the debugger + inspector, and the
   WASM sandbox — the dynamic module's whole shape, with the agentic
   seam's security floor (sandboxing, permission gates) now standing.
   The graph→WASM compiler (graphs compiling INTO the sandbox) is the
   natural next arc, alongside the editor surfaces that consume all of
   it. The
   full design remains at [dynamic-design.md](dynamic-design.md)
   — the third application module, shared by web and game: FFI plugins
   with hot reload for always-up processes (the trusted tier), the
   script-engine registry (v12 first, `script` stays its first customer),
   and Blueprint-style visual programming whose carrier is the `.hyg`
   graph format with Rust/JS/WASM compilers where **conversion IS
   validation** (errors carry node/pin coordinates and the flow chain —
   the graph's stack trace). Debugger + inspector with doc links serve
   both modules; route APIs as event graphs land in Hydrargyros Press.
   Marketplace seams designed now, publishing built later. **Decisions
   locked:** `abi_stable` at the FFI boundary (Rust-shaped signatures —
   raw C pointer protocols fight ownership), **wasmtime** as the WASM
   runtime (standards-compliant; cranelift is already in-tree via v12;
   emscripten-compiled C rides in *sandboxed*, no native ABI involved).
   The **agentic seam** — prompt → graph draft for enduser visual scan,
   generated regression suites and database scaffolding, ollama/zai/
   deepseek adapter clients — is designed but deliberately outside the
   milestones until the security floor lands: wasmtime sandboxing by
   default, encryption, and host-enforced policy gates (credential paths
   salted+hashed or rejected; nothing applies without the human scan).
   Policy gates decided: a **static screen over the graph document** —
   host-side floor, adapter capability limits as later hardening. The
   drafting arc is complete; implementation starts at dynamic M0 (the
   engine registry).

## The editor arc — Hydrargyros Studio

The game-editor milestone queue ([game-editor-design.md](game-editor-design.md)):
M0 scene model (**landed**), M1 editor core (**landed**), M2 manipulation
(**landed**), M3 play mode (snapshot/restore, live reload), M4 assets, M5
polish (packaging, editor camera controls). One binary — the editor *is* the
engine running with a shell around it; the headless-first pump, the
registered window backends, and the multi-consumer `FrameSink` are the
seams it grows through.

7. **Studio M0 landed (2026-11-14): the scene model.** Scenes are `.hyg`
   documents and everything about them runs and tests headless. The
   **carrier type is now code**: `hyg::carrier` (feature-free, crate root)
   — the terminal `.hyg` suffix is the type, the preceding suffix names the
   serde format, `.json.hyg` is the default (pretty-printed — scenes are
   diffable, mergeable, greppable), a bare `.hyg` sniffs as JSON, unknown
   suffixes (`.cbor.hyg` and friends) are explicit errors naming the
   formats this build carries, and the writer picks the format from the
   filename, never a content-type guess. The **scene document**
   (`hyg::game::scene`, feature-free) is `{scene, format, camera,
   entities}` with the graph model's contracts: a `SCENE_FORMAT` gate
   (migrations are the versioning path), unknown extra fields tolerated on
   load and dropped on regeneration, vectors as number lists read through
   the `SceneVec2/3/4`/`SceneQuat` vocabulary (the native `Var::Vec3`/
   `Quat` variants accepted too — the in-memory authoring path), all
   entity/transform fields optional with identity defaults, and the camera
   projection as `{"ortho": …}` / `{"perspective": …}` (`Projection`'s
   `FromVar`; absent camera = the engine default). **Components
   materialize through a registry** (`register_component_factory`/
   `create_component` — the backend-registry pattern): built-ins
   pre-seeded (`light`, `mesh_renderer`, `physics_body`, and `script`
   behind the `script` feature), user components register their own
   factories, unknown types are explicit errors naming what IS registered,
   and the `ComponentContext` carries the entity name (a body's default
   name) plus the base directory file-referencing params resolve against
   (the scene file's dir on load). **The component documents itself**:
   two new `Component` hooks — `scene_type` (the registry name; `None` =
   runtime-only: never materialized, skipped on save) and `scene_params`
   (the canonical params, defaults omitted — a default light documents as
   `{"type": "light"}`) — with `Entity::iter_components` feeding the save
   walk. Model-backed `MeshRenderer`s are runtime-only until the asset
   pipeline gives models provenance. **Loading is `FromVar` validation**:
   strict, path-tracked rejections (`"entities.0.components.0.mass":
   expected a number, found string`) — scene floats widen from ints
   (JSON's one number type; `0` loads as `0.0`), and non-finite floats
   write as `null` (serde_json's encoding) and then FAIL the load with the
   path — never a silent zero. **The editor's git contract holds**: save
   serializes the LIVE tree, and the bytes are a canonical function of
   typed state — floats print the f32's shortest decimal, which parses
   back onto the same f32 through the f64 hop, so build → save → load →
   save is byte-identical (three-generation test; precision-hostile values
   pinned: `0.1+0.2`, `8.89`, the `-1e-45` subnormal). **Scripts ride as
   file references**: the document names a script path, the factory loads
   it against the scene's base dir, `ScriptedComponent` records
   `source_path` and writes it back on save — the body is never inlined;
   inline-source scripts and custom hook names behave correctly (the
   former runtime-only, the latter round-tripping). `Scene::spawn_into`
   attaches the camera and roots to a live `Engine`. Tests: 18 in
   `tests/scene.rs`, 7 in `tests/carrier.rs` (suite 324 green). Next
   rung: **M1 — the editor core** (winit shell + egui panels + viewport
   presenting `FrameSink`).

8. **Studio M1 landed (2026-11-16): the editor core.** The design's open
   question — winit 0.30 + egui on the same surface — is answered as
   code, and the version fit was already sitting in the registry:
   **egui 0.37 depends on wgpu 30 and winit 0.30 exactly** (verified
   against the vendored manifests before adding; minimal features —
   `default_fonts`, no clipboard yet). The **`editor` feature** composes
   the trio with the engine's own `wgpu`/`winit`. The module splits for
   the headless-first rule: `hyg::game::editor::ui` (feature-free) is the
   editor's STATE and OPERATIONS — selection (tree paths
   `[root_idx, child_idx, …]`), hierarchy snapshots
   (`HierarchyNode` carrying the path that selects each entity back),
   and the inspector's edits (rename/enable/transform, applied through
   the new `Engine::entity_at(_mut)` path walk and `Entity::set_name`)
   — every op answers `false` and touches nothing on an empty or stale
   selection; `hyg::game::editor::shell` (the feature's payload) is the
   window: the pump drives the engine on a worker thread through a
   shared mutex, winit owns the main thread, and each display frame
   blits the engine's latest `FrameSink` frame (the native view's
   machinery, extracted to a shared `pub(crate) BlitView`) as an
   underlay pass, then composites egui panels in a second load pass of
   the same surface view — the Hierarchy (click to select), the
   Inspector (name/enabled/transform DragValues, writes land live), the
   status bar. The layering proof ran on the desktop: panels over the
   live physics scene, ball mid-fall under the panels. Panels lock the
   engine while drawing — a value drag can cost the pump a frame;
   correctness first, noted honestly in the shell docs. Debugger
   rulings recorded in the design doc (user-approved): breakpoint
   parking is INDEFINITE (no time limit — safe since the blocking-pool
   fix), pause-anywhere is in-scope, and a paused graph must never
   freeze the picture — the shell keeps drawing. **`DebugRun::pause`**
   landed with the ruling: arming a running graph stops it at the NEXT
   node entry (mid-execute included), tested with a slow function node
   under real concurrency. Engine surface additions:
   `entity_at`/`entity_at_mut`, `Entity::set_name`, `iter_components`
   made public (the inspector and the scene save share it).
   `examples/editor_shell.rs` runs the shell over the sandbox scene
   (`cargo run --example editor_shell --features editor,physics`). 5
   headless UI-logic tests in `tests/editor.rs` (suite 330 green under
   `--all-features`, clippy clean). Also: `cargo clean` reclaimed 261G
   of build cache — the linker had started dying (`ld: Bus error`) on
   the 100%-full data disk. Next rung: **M2 — manipulation** (picking,
   transform gizmos, undo/redo, scene save/load from the UI).

9. **Studio M2 landed (2026-11-16, overnight): manipulation.** The
   viewport is now an editor. **Picking** (`hyg::game::editor::pick`,
   feature-free): a click unprojects through the camera's inverse
   view·projection (a general Gauss-Jordan 4×4 inverse — both
   projections) into a world ray; each entity's pick proxy (a box: the
   shape-mode renderer's `half_size`, a unit cube for models) is
   slab-tested in its composed world transform; nearest wins, disabled
   entities are transparent, nested entities are hit where their parents
   put them. **Gizmos** (`hyg::game::editor::gizmo`, feature-free):
   screen-space T/R/S handles sized in PIXELS (grabbable at any
   distance), each axis measuring its own px-per-world-unit at the
   entity's depth so drags map back to honest world deltas along the
   entity's LOCAL axes; the translate center moves in the view plane;
   the rotate ring turns around the camera's forward (the y-down atan2
   sweep maps to the drag's visual direction — the test pins the
   contract); scale is additive per-axis. The shell paints the layout
   (colored axis lines, arrowheads, center square, ring) in a
   transparent CentralPanel over the letterboxed viewport — the pick
   viewport is the SAME rect the pixels live in. **Undo/redo**
   (`editor::ui::UndoStack`): the designed `(path, old, new)` command
   stack with per-drag coalescing — a 400ms window folds a drag's
   per-frame edits into ONE step whose undo returns to the gesture's
   start; new commands clear the redo branch; **Delete owns the removed
   subtree** (it sits on the undo stack, undo re-inserts it, redo
   re-removes — no Clone needed on `Entity`, which holds dyn
   components); Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z, Del deletes, and the
   bottom bar carries undo/redo buttons. **Scene save/load from the
   UI**: path field + Save/Load in the bottom bar — save writes the M0
   canonical document of the live tree through the carrier (byte-clean,
   format from the filename); load replaces the roots + camera and
   clears the undo history (stored paths moved with the old tree).
   Engine surface additions: `set_roots`, `remove_entity`,
   `insert_entity`, `world_transform_at`; Entity gained `remove_child`/
   `insert_child` (pub(crate)) and `set_name` (M1). **The Studio has a
   face**: a deliberate quicksilver theme (deep blue-gray bases,
   silver-blue accents, gold selection) — DARK DEFAULT, light palette
   riding along, toggle in the bottom bar (0.37's styles are
   theme-keyed: both palettes installed, the preference picks). FPS
   lands in the status bar. Tests: 17 in `tests/editor.rs` — the gizmo
   rendering assertion is GEOMETRY-level (a headless egui pass
   tessellates the painted gizmo; vertex count + every vertex inside
   the viewport; the pixel pass composites it in the shell — the
   done-when's "pixel tests" land honestly one level up, where the
   editor's own layer lives). Suite 220 core / 342 `--all-features`,
   clippy clean. Known limits, stated: editor camera controls
   (orbit/fly) are future work — picking runs through the scene camera;
   the rotate ring covers view-plane rotation only; gizmos size from
   proxy boxes until the asset pipeline knows real bounds. **Fix round
   (2026-11-16, after first hands-on review): edit mode arrives early.**
   The always-running pump reverted every position/rotation edit a frame
   later (rapier's step-and-sync owns transforms while the sim runs), so
   the pump's UPDATE pass is now gated behind a play flag — **paused by
   default (edit mode), play/pause button + Space, still drawing while
   paused** — M3's edit/play distinction pulled forward; snapshot-restore
   remains M3. The history panel landed Godot-style: the command
   timeline as a clickable list (applied rows solid, un-done dim, cursor
   gold) with `UndoStack::goto` traveling to any point. The scene
   save/load grew an **in-app file dialog** (the Godot way — a themable
   directory browser over `.hyg` documents, save names auto-repaired to
   the carrier suffix; no toolkit dialogs, no new deps). Inspector
   fixes: a ScrollArea (the component list was below the fold),
   a rename that actually renames (the buffer persists in editor state —
   rebuilt-from-entity each frame ate every keystroke — committed on
   blur/Enter), and per-component param summaries. The hierarchy's
   clickable Label became a `selectable_label` (the old one dragged out
   text selections that stuck). All UI glyphs de-tofu'd to ASCII — the
   default font carries no ↶/☀/☿/▶ and font-error boxes read as bugs;
   a proper icon font ships with the theme work. 20 editor tests
   (history travel, delete naming, suffix repair); suite 222 core / 345
   `--all-features`, clippy clean. **Round 2 (same day, second
   hands-on review): the chrome takes shape.** The gizmo alignment bug
   was a viewport mismatch — the pick/gizmo math used the central
   panel's rect while the blit letterboxes into the WHOLE SURFACE, so
   clicks selected one thing while the pixels showed another; the
   viewport now derives from the surface letterbox (`surface_viewport`),
   exactly matching the pixels. Resize sluggishness was the paused pump
   re-rendering every 16ms (starving the UI thread of the engine lock):
   the pump re-renders only on a scene-dirty flag while paused, and
   Resized requests an immediate redraw. A **menu bar** (File:
   New/Open/Save/Save As/Quit; Edit: Undo/Redo + History; View: theme)
   and a **toolbar** (transport + gizmo modes + **scene tabs** + fps)
   sit above the viewport; the bottom bar is a status line. The
   transport is complete: **Play snapshots the scene's document Var,
   Stop restores it (play never dirties edits — M3's snapshot-restore
   pulled forward), Restart replays**. Scene tabs carry dirty marks and
   switch by loading their document. Hover-highlighting is dead
   globally (`interaction.selectable_labels = false`), the history
   window resizes on both axes, and the inspector's vectors fill their
   rows (3-in-a-row for vec3, 2×2 for the quaternion, shrinking with
   the panel). **Round 3 (same day, third review): the editor gets its
   own eyes and hands.** The **editor orbit camera** landed
   (`editor::camera_ctl`, feature-free): right-drag orbits, wheel
   zooms, middle-drag pans — and the viewport's view is now distinct
   from the SCENE's camera (edit = orbit, save/play = the scene
   camera; editing the view never dirties the document). Stacked-perspective
   gizmo grabs are fixed (overlap tie-break toward the axis more
   perpendicular to the view, foreshortened axes shrink, every arrow
   lettered X/Y/Z), and `view_basis`'s screen contract is TEST-PINNED
   after a would-be fix flipped a cross product and broke every
   view-plane drag — the empirical test now guards it forever. The
   transport moved to the menu bar's far right, color-coded; Delete
   sits under Edit; File > Quit works (a shared exit flag). Creation
   flows: **+ entity** (child of the selection or root) and **+
   component** (the scene registry's types, default params — script
   waits for the script editor). Inspector axis letters are
   color-coded to match the gizmo, the side panels resize over a
   continuous range, and a 16px scene bar under the viewport names the
   loaded document. New Scene/tab switches stop play and clear the
   play snapshot (restart used to replay the previous scene's state).
   22 editor tests; suite 224 core / 347 `--all-features`, clippy
   clean. **Round 4 (fourth review): the gizmo mystery closed.** An
   end-to-end test (layout → delta → projection) caught the real bug:
   an axis pointing at the lens collapses toward 0 px/unit and the raw
   floor turned a 30px drag into 600 world units toward the camera —
   reading as reversed. The drag scale now clamps to ≥15% of the
   healthiest axis, and the default view flipped to the industry
   convention (orbit yaw π: world +x renders screen-RIGHT). The orbit
   drag sign is now view-invariant (drag right = the eye along the
   camera's CURRENT screen-right — the old sign inverted at the default
   view). Orthographic cameras get a proper **2D gizmo** (screen-aligned
   X/Y at uniform scale). The transport sits on the menu bar's far
   right in the user's color language (play green, stop red, restart
   orange; move/rotate/scale red/green/blue), the hierarchy grew
   **drag-to-reparent** (cycle-refusing `Engine::reparent` with
   sibling-shift adjustment) and click-again-deselect (viewport too),
   and the bottom bar became the **dock**: Log (status history) | Stats
   (live counts) | Console | Debugger | Profiler (staged for their
   milestones — the dock is the extension point). The inspector stopped
   ballooning (a `desired_width(INFINITY)` rename field was maxing the
   panel — that was the phantom inside-the-panel resize line), and
   "+ entity" is an obvious green +. 24 editor tests; suite 226 core /
   349 `--all-features`, clippy clean. **Round 5 (fifth review).** The
   inspector's slow ballooning was reproduced by a headless probe that
   drives the REAL panel for 240 frames (bisected to the transform rows:
   field widths derived from `available_width` chase the panel's own
   chrome a few px a frame) — constants can't feed back, and the probe
   test guards the contract. The viewport gained its **projection
   switch** (View menu + a view-options button top-right; wireframe/
   normals/AO staged for M5) — the orbit camera has an ortho mode
   (half-extents from the distance, one zoom knob), and the **2D
   ortho gizmo is test-pinned** (two screen-aligned axes, 1:1 drags).
   + component now spawns ready components (UI-side default params;
   factories stay strict). And the file dialog went **native** (rfd,
   xdg-desktop-portal on Linux — favorites, locations, filtering for
   free; the in-app browser is deleted). 26 editor tests; suite 227
   core / 351 `--all-features`, clippy clean. **Round 6 (sixth
   review): the CAMERA COMPONENT and 2D/3D scenes.** Scenes now declare
   their dimensionality through a **main camera** — an entity-attached
   [`CameraComponent`](game::camera_component) (ortho = 2D, perspective
   = 3D; registered as "camera", full save/load params). Play resolves
   the main camera and renders through it — a scene without one plays
   BLANK, per contract; the editor viewport renders through its own
   orbit (edit views are the editor's, never the document's), and new
   scenes spawn a main camera entity (ortho default). The **viewport
   renders at the panel's size**: a shared frame-size gate drives
   `WgpuRenderer::set_size` (target + depth + readback recreated) and
   the blit gained a sub-rect fit (extents + center in the uniform) —
   the frame lands exactly on the panel, no letterbox bars anywhere.
   The **camera preview PiP** landed: while a camera entity is selected
   and its TRS edited (or locked), the pump renders its view into sink
   tag 1 (FrameSink tagged slots; `latest_since_tag`) and the shell
   uploads it as an egui texture in a closable window. The inspector
   carries camera controls (main, projection, fov/half-extents/near/
   far) and the **editor camera accordion** (target, distance,
   yaw/pitch, fov — view state, never saved). + component spawns ready
   components (UI-side default params). 28 editor tests; suite 229 core
   / 353 `--all-features`, clippy clean. Next rung: **M3 — play mode**
   (script live reload, console, per-scene snapshots tied to tabs). **Round 7 (2026-11-16, seventh review): the black box and the PiP.**
   The giant black box in the viewport's lower half was an off-by-a-
   full-screen shift in the blit's clip-center formula (`1 - (2y/sh -
   1)` should be `1 - 2y/sh` — pixel y is down, clip y is up; the extra
   `+1` pushed the whole blit a screen height up, leaving the panel
   black). The camera PiP never appeared from a chicken-and-egg in the
   visibility logic (it required a seen preview frame that required the
   preview rendering that required the visibility) — now: camera
   entity selected AND (its TRS edited OR locked OR within a 1s grace)
   AND the user hasn't closed it. The **camera component defaults to
   3D** (perspective, framed on the origin) and **new scenes are EMPTY
   except the main camera entity** — the camera decides the scene's
   dimensionality, defaulting to 3D per review. **Sub-scenes answered
   in the design doc**: composition, not a new container — a scene
   reference component + per-camera render targets (the PiP's tagged
   slot is that seam already); "rendered first in view" is draw-order
   policy. The example scene carries its camera entity (play renders
   the demo view). 29 editor tests; suite 230 core / 354
   `--all-features`, clippy clean. **M3 landed (2026-11-16, same-day
   follow-through): play mode.** The editor opens **maximized and
   focused** (also fixing the screenshot feedback loop). **Script live
   reload** — the design's save-equals-reload: the editor polls script
   files' mtimes (~1/sec) and a changed file swaps the component's
   source through `ScriptedComponent::reload_from_file` (`Script::load`
   — same id, host closures re-bound, script state resets per the v12
   contract); a broken reload refuses at the boundary and the component
   fails loud on its next hook (both pinned by tests, including the
   heal-after-break cycle). The **pump catches script panics**
   (catch_unwind around the frame): the console shows the error, the
   sim drops to edit mode, and the editor stays alive — the
   poison-tolerant lock keeps the engine usable. The dock's **Console
   tab is real**: script panics (red) + editor statuses, newest last,
   stick-to-bottom. The transport completes with **step** (one frame
   while paused). **Per-tab live state**: switching tabs snapshots the
   outgoing tree into its tab and restores the incoming tab's — unsaved
   edits survive switching; tabs are independent in-memory scenes. The
   editor example carries a scripted drifter + patrol.js (the live
   reload demo: edit while playing; break it, watch the console).
   required-features gained `script` (the editor's scripting story rides
   the same seam). 31 editor tests; suite 230 core / 356
   `--all-features`, clippy clean; screenshot-verified maximized with
   the scene filling the viewport. **Round 8 (2026-11-16): the gizmo
   respects zoom.** The gizmo's handles were fixed 90px screen
   constructs — only their direction came from the world axes, so
   zooming rescaled the scene while the gizmo stayed put ("doesn't
   respect the zoom", per review). The handles are now TRUE MVP
   projections of world points (entity + axis × `GIZMO_WORLD_LEN` = a
   world-locked 1.4 units): the gizmo scales with the viewport's zoom
   like the geometry, and the hit-test/drag chain follows the projected
   tips (picking keeps its inverse-MVP path — that part was always
   correct; a distance-scaled world length would have canceled the
   zoom into a constant screen size — caught by the new zoom-respect
   test before it shipped). M4 — assets (import flows, debug views) —
   is the next rung.

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
