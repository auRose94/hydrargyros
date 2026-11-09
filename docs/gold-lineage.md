# hyg — heritage: the gold lineage

hyg is learnable standalone — the README, roadmap, and code comments assume no
knowledge of anything else. This doc is the one place that records where hyg
came from, for archaeology: when a contract question needs the historical
answer (why a piece mirrors its ancestor, or diverges on purpose), look here.

The ancestor is **gold**, a C++20/26 framework (core in `src/`, game module in
`include/game/` + `src/game/`, local checkout at `/mnt/Matrix/Work/gold`).
hyg is **not** a branch of it — it is a separate project that reimagines the
same thesis in Rust, free to diverge, and gold stays untouched.

**This doc is closed history.** The port is complete (the roadmap's rung queue
closed); new work branches from the roadmap sections, not from gold. New code
and comments should not reference gold — this page is their replacement.

## The name

gold is **Au**. Hydrargyros is *hydrargyrum* — Greek, "water-silver" — the
element mercury, **Hg**, symbol ☿. Both chemical-element metals: the pairing
is a joke *and* a signal. The planet and the swift messenger god fit the
"fast prototyping" goal. The crate/namespace is `hyg` (short, and distinct
from `hg`, which is the Mercurial VCS binary).

## Module map — ancestor → hyg

| gold (C++) | hyg (Rust) | Notes |
| --- | --- | --- |
| `var` / `object` / `list` | `src/var.rs`, `src/object.rs`, `src/list.rs` | COW dynamic value, prototype chain, insertion-ordered list |
| `jo!` / `ja!` | `o!` / `a!` | Renamed: `o` reads as "object" instantly; the `j` only dodged C++/HTML keyword clashes |
| implicit `v["k"] = 5` assignment-conversion | explicit `.get()` / `.put()` / `.set()` | The deliberate refusal — the ancestor's footgun is unrepresentable under borrow checking |
| `varRef::operator=` | `VarRef` proxy `.put()` | Mutates the shared value in place |
| hand-rolled JSON/BSON/CBOR/MsgPack/UBJSON | one serde impl on `Var` | Any serde format works for free |
| `web::server` (uWS) | `src/web/http/` (axum) | uWS streaming internals (`cork`/`onWritable`/`writeContinue`) don't map to hyper — divergence |
| `iHTML` / `Parser` | `src/web/html.rs` | Void tags, entity decoding, implicit closes, malformed-input recovery |
| `style.cpp` (CSS, `easeValue`) | `src/web/css.rs`; easing concept lives on in `game::tween::Easing` | |
| web store / database | `src/web/store.rs`, `src/web/database.rs` | `$set`/`$unset`/`$inc`, `$in`, Mongo-shaped ids; `"file"` backend only, like the ancestor |
| `webTest.cpp` | `tests/` | Integration-style, one file per area |
| `myWebProject` | `examples/web_site.rs` | Multi-page demo homage |
| engine `start()` pump | `src/game/engine.rs` | Same frame: poll events → sort + init → priority `update(dt)` → draw → frame cap |
| component priority tiers | `src/game/component.rs` | high/data/physics/generic/draw + custom values |
| entity / transform | `src/game/entity.rs`, `src/game/transform.rs` | Parent-first composition scene tree |
| `windowSystemHeadless` + backend registry | `src/game/window.rs` (+ `winit_window.rs`, `stream_window.rs`, `native_view.rs`) | Events cross as `Var` |
| physics `world` / `physicsBackend` (bullet) | `src/game/physics*.rs` (`none` + rapier backends) | Step-and-sync; `mass` proto knob (inertia at given mass, the `calculateLocalInertia` contract); **no joints** — the ancestor never constructs a constraint anywhere, so nothing was ported (rapier's `ImpulseJointSet` is the designated future slot); `debugDraw` was render-coupled and untested even in the ancestor — not ported |
| `camera.cpp` `setViewTransform` (bx `mtxLookAt` + `mtxProj(…, homo=false)`) | `src/game/camera.rs` | z ∈ [0, 1] after the divide — WebGPU's clip convention; one cull mode kept correct via the right-handed look-at variant (divergence); `LIGHT_COUNT=1` → 8 |
| `light` component + KHR punctual math | `src/game/light.rs` + the wgpu light uniforms | Four uniform rows per light; range attenuation + smoothstep spot cones |
| `audioSystem` (SDL adapters as plugins) | `src/game/audio.rs` (rodio) | The registry ships a `none` fallback where the ancestor returned nullptr on a miss; `play` takes the source directly; `stop(id)` names one sound where the ancestor's `stop()` stopped everything |
| `script` facade (`include/lang/lang.hpp`) | `src/script.rs` (v12) | JSON bridge, thread-local context pool, `ScriptedComponent` lifecycle |
| `sslCert` / `sslKey` | `Server` HTTPS config pair | rustls transport, verified by a real handshake in tests |
| screenshot debug aid | offscreen render → PNG save | Render tests assert actual pixels |

## Divergences on purpose

- **No implicit assignment-conversion.** Explicit reads/writes everywhere;
  `Null` as a literal; typed reads via `Into<…>` / `as_*()`.
- **`set_gravity` is a LIVE knob** (wakes every body — settled bodies sleep).
  The ancestor's bullet backend only set gravity at world init, so a
  mid-run flip silently no-op'd on resting scenes.
- **`BodyType` keeps static/dynamic explicit**; `mass` on a static body is
  ignored. The ancestor collapsed the distinction into `mass != 0`.
- **The quad scene stays an orthographic image of the world** even under a
  perspective camera (mapping through the ortho window derived at the target
  plane). The ancestor projected everything through the one camera.
- **`OrbitController` is hyg-native** — the ancestor's camera rode its script
  seam and shipped no controller.
- **Tweens/timers are hyg-native.** The ancestor's only easing was the UI
  style system's CSS timing functions (`style.cpp` `easeValue`).
- **serde over hand-rolled formats; axum over uWS; rapier over bullet;
  rodio over SDL; v12 over the ancestor's interpreter.** Ecosystem wheels,
  not reinventions.
