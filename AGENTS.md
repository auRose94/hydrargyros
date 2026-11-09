# AGENTS.md — Hydrargyros ☿

Instructions for AI coding agents working in this repository.

Hydrargyros (crate `hyg`) is a Rust reimagining of **gold** — a C++20/26
framework whose thesis is *high-level, JavaScript-like ergonomics for
prototyping without sacrificing speed*. It is a standalone project, **not** a
gold branch. The checkout directory is `/mnt/Matrix/Work/mercury` (kept from
the pre-rename era; the project and all imports are Hydrargyros /
`hyg::`). The repo is **public** — never commit secrets or
machine-specific paths.

## Source of truth

**Read `docs/roadmap.md` first.** It is the plan and the architecture-decision
record (why `Arc<Mutex>`, why serde, why not Bevy) and is updated in the same
change that lands work. If your change completes a roadmap item, update its
state there in the same commit.

The original C++ being ported lives at `/mnt/Matrix/Work/gold` (core in
`src/`, game module in `include/game/` + `src/game/`). When porting an API,
check gold's version first — contracts are deliberately mirrored (pump order
from `engine.cpp start()`, the physics backend split, the `none`-backend
fallback).

## Build & verify

```
cargo test                  # core (feature-free)
cargo test --all-features   # full suite: web, physics, wgpu, stream, gltf, winit, tray
cargo clippy --all-targets
```

- Known pre-existing failures: 4 doc tests (`web/store`, `web/database`,
  `web/html`, `from_var`) — verified identical on pristine HEAD. Out of scope
  unless your change touches web; don't chase them.
- **Do not run `cargo fmt` across the repo** — it reformats ~40 unrelated
  files. Format only what you touch. `rustfmt` on a file with `mod`
  declarations (e.g. `src/game/mod.rs`) recursively formats every
  submodule — the same collateral in disguise; pass only leaf files.
- **`clippy --fix --tests` fixes lint findings repo-wide**, not just your
  diff — after running it, `git status` and revert anything outside your
  change set before committing.
- Feature gates keep the core lean: `web`, `physics`, `wgpu`, `winit`, `tray`,
  `stream`, `gltf`, `image`, `script`, `audio`. New heavy dependencies go
  behind a feature, and feature-gated examples need
  `[[example]] required-features` in Cargo.toml.
- `Cargo.lock` is untracked (library crate); `/public` is ignored.

## Architecture invariants (do not break)

- **Everything is `Arc<Mutex<…>>` and multithreaded.** The `Var` value graph
  is `Send + Sync` (compile-time assertion in `src/lib.rs`); locks go through
  the poison-tolerant `crate::lock`. Never introduce `Rc`/`RefCell`.
- **serde is the serialization layer** — one `Serialize`/`Deserialize` impl on
  `Var`, so any serde format works for free. `Func` serializes as the handle
  `{"__func__": id}` and round-trips through the process-global id registry
  (unknown ids degrade to `Null` — closures can't cross processes).
- **Explicit reads/writes.** `.get()`/`.put()`/`.set()`, typed reads via
  `Into<…>`/`as_*()`, `Null` as a literal. Don't add implicit
  assignment-conversion — that's the gold footgun this design exists to
  avoid. Objects are insertion-ordered (`indexmap`) with a prototype chain.
- **Builders only where data is a tree** (HTML). No CSS builders (CSS is
  `o!{}` + the parser) and no JS builders (scripting comes through `func!` +
  the interpreter seam).
- **Headless-first game module.** The whole engine loop must run and test
  without a GPU or window; the wgpu renderer is offscreen and render tests
  assert actual pixels. The `none` physics backend keeps its no-op contract
  (transforms stay under script control).
- **Not Bevy, don't reinvent wheels.** No ECS integration; use serde, axum,
  wgpu, rapier, tokio rather than hand-rolling replacements.

## Hard-won technical traps

- **`Quad.size` is HALF-extents** (shader corner offsets are ±1.0). The camera
  path transforms BOTH quad position and size (`size /= camera.half_extents`)
  — omitting the size transform made every quad render giant, reading as a
  blank screen.
- **WebGPU clip space is z ∈ [0, w], not GL's [−1, w].** The camera matrix
  must map the depth window's near plane to 0; GL-style mapping silently
  clips the entire near half.
- **Pixel tests: sample quad edges/corners, not centers** — a center sample
  is invariant to scale bugs.
- **Dependency versions are API-breaking vs tutorials** (rapier3d 0.36, wgpu
  30, winit 0.30, tray-icon 0.26). Verify APIs against the vendored source in
  `~/.cargo/registry/src/` before writing code.
- Web server `stop()` is watch-based with a polled grace drain;
  axum-server's notify_waiters can be missed between accept-loop iterations —
  if shutdown hangs reappear, suspect lost-notify first. Never double-spawn
  `start()` on one `Server`.
- `o!` returns an `Object` (wrap in `Var::from`); verify batch text edits
  with grep afterwards — blind `str.replace` no-ops silently on needle
  mismatch.
- When a change alters two things at once and the result confuses you, revert
  one to isolate (this pattern ended several multi-day debugging sagas).
- The `script` feature's v12 git dep (DevYatsu/v12, pinned rev) cannot
  `cargo fetch` on a clean machine — upstream's tree carries a `test262`
  gitlink with no `.gitmodules` mapping, so cargo's submodule update fails.
  This machine's cargo git checkout is pre-seeded; on a fresh clone, that
  submodule error is the tell (upstream fix pending — see roadmap).
- Clippy without the feature set lies about gated code: an import used only
  under a feature reads as "unused" and removing it breaks `--all-features`.
  Gate the import with `#[cfg(feature = …)]` instead of deleting it.

## Conventions

- Conventional commits with module scope: `feat(game): …`, `fix(web): …`,
  `feat(core,game): …`.
- Comments explain the gold lineage — when a piece mirrors gold, say so
  ("gold's pump + dispatch", "gold's sslCert/sslKey"). Match that voice.
- Fix stale `mercury::` references in docs/prose when you touch them.
- Tests are integration-style in `tests/`, one file per area (ported from
  gold's tests where applicable), and drive real behavior: real HTTP and
  WebSocket traffic, real TLS handshakes, real pixels. Time-bound every test
  `await`.
- Visual deliverables (streaming page, tray glyph, camera framing) are
  user-reviewed: render them and verify visually, don't just assert success.
