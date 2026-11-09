# hyg — platform support

hyg's invariant is **headless-first**: the engine loop, web server, parsers,
store, physics (`none` backend), tween, and the script seam run and test
without a GPU, window, or GUI toolkit anywhere Rust compiles. The heavy
backends ride cross-platform wheels (wgpu, winit, axum, rodio, rustls,
tray-icon), so portability is mostly about the seams where an OS peeks
through.

## Support matrix

| Area | Linux | Windows | macOS | Notes |
| --- | --- | --- | --- | --- |
| Core (`Var`/`Object`/`List`/serde/`FromVar`) | ✅ verified | ✅ pure Rust | ✅ pure Rust | No OS surface at all |
| Web siblings (`html`/`css`/`store`/`database`) | ✅ verified | ✅ | ✅ | File I/O via `std::fs` + `temp_dir()` |
| Web server (`web` feature, axum/tokio/rustls) | ✅ verified (real traffic + TLS) | ✅ expected | ✅ expected | Binds TCP; no Unix sockets anywhere |
| Physics — `none` backend | ✅ verified | ✅ pure Rust | ✅ pure Rust | The no-op contract keeps transforms under script control |
| Physics — rapier3d | ✅ verified | ✅ pure Rust | ✅ pure Rust | Deterministic fixed timestep headless |
| Render — offscreen wgpu | ✅ verified (pixel tests) | ✅ expected (DX12) | ✅ expected (Metal) | Vulkan/GL fallbacks; PNG readback needs no surface |
| glTF + textures (`gltf`/`image`) | ✅ verified | ✅ pure Rust | ✅ pure Rust | |
| Game — headless loop + tween | ✅ verified | ✅ | ✅ | The whole pump runs windowless |
| Audio (`audio` feature, rodio/cpal) | ✅ tests use the `none` recorder | ✅ expected (WASAPI) | ✅ expected (CoreAudio) | Contracts test without a device; real device plays where cpal does |
| Scripting (`script` feature, v12) | ✅ verified | ✅ expected | ✅ expected | ⚠ the pinned v12 git dep needs a seeded cargo checkout on any OS (upstream `test262` gitlink wart — see roadmap) |
| Window — `winit` backend | ✅ (X11/Wayland) | ✅ expected | ⚠ | winit runs its event loop on the main thread everywhere; hyg's pump is off-thread through the same seam |
| Native view (borderless fullscreen hand-off) | ✅ (X11 any-thread escape hatch) | ✅ expected (off-main loops allowed) | ❌ refused honestly | macOS requires the event loop on the main thread; the hand-off prints a clear refusal instead of panicking |
| Streaming window (`stream`) | ✅ verified (WS traffic, QR decode) | ✅ expected | ✅ expected | Browser page + WebSocket — OS-neutral except `open_in_browser` |
| `open_in_browser` | ✅ `xdg-open` | ✅ `cmd /c start` | ✅ `open` | All three wired |
| Tray (`tray` feature) | ⚠ needs GTK (degrades gracefully without it) | ✅ expected (native tray-icon) | ✅ expected (native tray-icon) | Deps are target-split in Cargo.toml — verified by resolution: on Windows/macOS targets the graph carries tray-icon's native backend and zero GTK; Linux wires tray-icon → libappindicator → GTK. `gtk::init()` failure prints "tray unavailable" and continues |

Legend: ✅ verified · ⚠ works with caveats · ❌ explicit, honest refusal.
Three verification tiers: **verified on Linux** (the test suite runs there),
**compile-verified on Windows** (`cargo check --all-features` against
`x86_64-pc-windows-gnu` — every cfg-gate, API use, and dependency resolves),
and **expected** (macOS, and Windows runtime behavior).

## Verification status

Everything marked **verified** runs in this repo's test suite on Linux
x86_64 — including real HTTP/WebSocket traffic, a real TLS handshake, real
GPU pixels asserted from PNG readback, and the QR round-trip decode.

**Windows x86_64 is compile-verified (2026-11-09):**
`cargo check --all-features --target x86_64-pc-windows-gnu` passes — the
whole graph builds for the target, including v12's interpreter stack,
wgpu's Windows deps, rodio (WASAPI), rapier, axum/rustls, and tray-icon's
native backend with zero GTK. The check runs under the
**mingw-w64-rust-bin** toolchain (Arch AUR, `/opt/rust`, rust 1.96.1 —
a separate sysroot; it cannot be mixed with the system rustc):

```
PATH=/opt/rust/bin:$PATH /opt/rust/bin/cargo check --all-features --target x86_64-pc-windows-gnu
```

Two honest boundaries: `cargo check` validates types, cfg-gates, and API
usage — it does not link or run (linking wants `x86_64-w64-mingw32-gcc`,
which is installed; running wants Windows or CI). And macOS stays
**expected, not verified** — the `aarch64-apple-darwin` target std is not
installed on this machine.

- **Dependency resolution per target** (manifest-level proof):
  `cargo tree --all-features --target x86_64-pc-windows-gnu -i gtk` is
  empty and tray-icon resolves to its native backend.
- **cfg-gate audit**: platform peeks live in exactly three files
  (`stream_window.rs` `open_in_browser`, `native_view.rs` main-thread
  gates, `tray.rs` GTK init), all with honest degradation paths.

Runtime verification wants real machines or CI runners — the natural
follow-up is a GitHub Actions matrix (linux/windows/macos, `cargo test`
core + `cargo check` all-features per target).

## Portability rules for new code

- **No hardcoded `/tmp`** — use `std::env::temp_dir()` (Windows has no
  `/tmp`). Same for any absolute path assumptions.
- **No Unix-only APIs** — no `std::os::unix`, no Unix domain sockets; the
  web layer speaks TCP.
- **OS peeks get cfg-gates with honest degradation** — follow the existing
  patterns: `open_in_browser`'s three-way gate, the native view's printed
  refusal, the tray's "unavailable, continuing" path. A missing desktop
  service is a warning, never a panic.
- **GUI toolkits are lazy** — GTK init happens only when a tray is actually
  requested, so headless machines never load it.
