# Examples — the feature tour

Every feature area is reachable from a runnable example. Feature-gated ones
name their `--features`; the feature-free ones run on a plain
`cargo run --example …` (and on any platform — no GPU, window, or device
needed).

| Example | Features | The tour |
| --- | --- | --- |
| `core_tour` | — | The value layer: `Var` literals, prototype chains, shared-mutable writes through `VarRef`, typed reads, list helpers, `tpl!`, serde, `Func` handles, `#[derive(FromVar)]` |
| `web_content` | — | The always-on web siblings: HTML builder + parser round-trip, CSS rules with `!important` flags, the file store (`$set`/`$inc`, `$in`), the `database` facade |
| `game_tween` | — | `Easing` curves, `Tween` step-and-land, drift-free repeating `Timer`, one-shots |
| `headless_game` | — | The engine pump with no window or GPU: priority-tiered components, events as `Var`, `FromVar`-derived config, settings |
| `script_seam` | `script` | v12 underneath: construct + call scripts, host functions in as `Var::Func`, globals round-trip, `ScriptedComponent` drives an entity through the transform seam |
| `game_world` | `physics`, `wgpu` | rapier behind the backend registry, step-and-sync, raycasts, and the offscreen renderer saving a screenshot |
| `game_lights` | `wgpu` | All three light kinds (directional/point/spot) at entity transforms, KHR punctual shading, offscreen PNG |
| `game_gltf` | `wgpu`, `stream`, `gltf` | A glTF model through the perspective camera + orbit controller, streamed to the browser |
| `game_audio` | `audio` | The backend registry (rodio → `none` recorder fallback), `Tone` synthesis to WAV bytes, per-play ids, master volume |
| `game_tray` | `tray` | The ☿ system-tray glyph with a Quit menu; headless machines degrade gracefully |
| `game_stream` | `physics`, `wgpu`, `stream` | The streaming window: the render buffer becomes a browser page, browser input comes back as engine events, QR hand-off |
| `game_window` | `physics`, `wgpu`, `winit` | The classic desktop window backend |
| `game_sandbox` | `physics`, `wgpu`, `stream`, `script`, `audio` | Everything at once: physics with a live gravity flip, a scripted patroller, lights, tweens, audio, orbit camera — streamed to the browser |
| `web_demo` | `web` | Routes as `Var -> Var` closures, one shared `Var` of state, JSON via serde |
| `web_site` | `web` | A multi-page site: HTML builder pages, form handling, the error hook, static mounts |

Headless-first by design: `core_tour`, `web_content`, `game_tween`,
`headless_game`, `script_seam`, and `game_world` (with the `none` physics
backend) run anywhere, including CI containers.

## Quick taste

```sh
cargo run --example core_tour
cargo run --example web_content
cargo run --example game_tween
cargo run --example headless_game
cargo run --example game_lights --features wgpu        # writes a PNG
cargo run --example game_sandbox --features physics,wgpu,stream,script,audio
```
