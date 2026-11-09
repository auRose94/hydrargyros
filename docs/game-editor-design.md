# Hydrargyros Studio — a unified game engine & editor

**Short name: `hyg` Studio** (the crate/namespace, pronounced however you
like — high-g, hey-g, hedge, hegg).

**Status: draft for review — not a commitment.** This sketch describes what a
Godot/Unreal-style editor for the game module could look like while staying
hyg-native. Nothing here exists yet; milestones are proposals.

## Vision

One binary: the editor *is* the engine running with a shell around it
(the Godot model, not the "editor generates a separate runtime" model).
hyg's headless-first design makes this natural rather than heroic:

- The pump is already drivable — `Engine::update(dt)` / `Engine::draw()` are
  public, so an editor can **pause, single-step, and scrub time** by driving
  frames itself. Play mode just hands the pump back its own loop.
- Window backends are registered by name. The editor is one more
  `WindowSystem` (an editor viewport), exactly like `headless`, `winit`,
  and the streaming window before it.
- The render buffer already has multiple consumers: the streaming window
  serves it to browsers and the native view blits it fullscreen — both feed
  from the same `FrameSink`. The editor viewport is consumer #3.
- Scenes are data. Entities, transforms, and events already cross the
  boundary as `Var`; serialization is serde. A scene file is a serialized
  `Var` tree — diffable, mergeable, testable without a GPU.

The thesis carries over: **prototype in `Var`/script ergonomics, then derive
the typed hot path.** The editor is where that loop lives — write a
`ScriptedComponent`, watch it move in the viewport, tune numbers in the
inspector live, then `#[derive(FromVar)]` the hot structs when the prototype
hardens.

**The pitch is accessibility.** Everything follows established game-
development principles on industry-established specs, so nothing here is a
bespoke format or an in-house language to learn: the scripting is JavaScript
(v12), the primary data container is JSON (with binary serde upgrades where
density matters — see the `.hyg` carrier type below), and the authoring loop
is Unity-like — entities, components, an inspector, scene files. The
potential is the most accessible engine to pick up *because* it reinvents
the least.

## Non-goals

- Not a DCC tool: no mesh editing, no texture painting (glTF import stays
  the asset path; Blender remains Blender).
- Not an ECS, and not Bevy — the `Arc<Mutex>` + trait-component model is
  the invariant.
- No closed proprietary scene format: scenes are serde data, readable
  without the editor.
- v1 is single-user, single-window. Collaborative editing is out of scope.

## Architecture

### Project & scene model (headless, testable first)

```
my_project/
  project.json.hyg   # name, window/render/audio backend config, physics
                     # settings, asset registry
  scenes/
    menu.json.hyg    # serialized entity tree (see below)
    level_1.json.hyg
  scripts/           # v12 script sources, one per ScriptedComponent
  assets/            # glTF, textures, audio — imported, content-addressed
  store/             # optional web-store data for hybrid projects
```

A scene file is an object per entity:

```json
{
  "name": "stack_block_3",
  "enabled": true,
  "transform": { "position": [0, 2.1, 0], "rotation": [0, 0, 0, 1], "scale": [1, 1, 1] },
  "children": [],
  "components": [
    { "type": "physics_body", "shape": "box", "half_extents": [0.5, 0.5, 0.5], "mass": 1.0 },
    { "type": "script", "source": "scripts/patroller.js" },
    { "type": "mesh_renderer", "model": "assets/crate.glb", "texture": "assets/crate.png" }
  ]
}
```

Loading maps it onto `Entity`/`Transform`/components; saving serializes the
live tree. `FromVar` is the load-side validator — a scene that doesn't
match a component's schema fails with the path-tracked error
(`"components[0].mass": expected number, found string`), never silently.

**Component reflection:** built-in components expose a
`ComponentDescriptor` (name, fields with types, defaults, ranges). The
descriptor drives three things from one source: scene (de)serialization,
inspector UI generation, and gizmo behavior (a light shows its cone, a
body shows its collider). User components are v12 scripts + a `Var` props
object, so they get the same inspector for free — the props object is just
a `Var` the inspector already knows how to edit.

### The `.hyg` carrier type

A `.hyg` file is a serialized `Var` — one file type for every structured
hyg document (scenes, project configs, theme templates, exported stores).
The **terminal `.hyg` suffix is the type** (one file association, one icon,
one "Open with Studio"); the **preceding suffix names the serde carrier
format**: `.json.hyg` is the default and the human-readable home for most
files; `.cbor.hyg` / `.bson.hyg` are the dense form for heavy scenes, where
binary containers parse faster and ship smaller. Anything serde can parse
qualifies — that's the point of "one `Serialize`/`Deserialize` impl on
`Var`, any format for free", surfaced as a naming convention instead of a
hidden choice.

Loading rule: strip `.hyg`, match the format suffix against the known
serde formats, and sniff-fallback to JSON (a bare `.hyg` file is JSON —
the accessible default). Writing rule: the writer picks the format from
the requested filename, never from content type guesses. JSON stays the
primary container (diffable, mergeable, greppable — scenes are code-adjacent
data); the dense carriers are a per-file upgrade for size, not a project-wide
leap of faith.

### The editor shell

- **Process layout** mirrors the winit backend: winit event loop on the main
  thread, engine pump on its own thread, both talking through the existing
  event seam (`Var` events in, `FrameSink` frames out).
- **Viewport** = the engine's renderer presenting into a sub-surface of the
  editor window (wgpu surface view; the offscreen path stays for tests).
  Camera preview: any `Camera` entity can be "looked through" — the
  viewport temporarily uses its projection (the orbit controller stays for
  free-fly editing).
- **UI framework: egui** (immediate mode, wgpu renderer, docked panels) —
  the "right existing wheel" per the invariant; it renders into the same
  frame as the viewport. Panels: **Hierarchy** (entity tree, drag to
  reparent — transform composition makes this correct by construction),
  **Inspector** (descriptor-generated fields, live-updating during play),
  **Assets** (project directory browser + import), **Console** (engine log,
  script errors — script throws already fail loud), **Physics/Audio
  views** (bodies, raycasts, playing sounds via the `none`/rodio worlds).
- **Gizmos & overlays:** an overlay pass over the frame (the quad pipeline
  already draws SDF-clipped shapes in list order — translation/rotate/scale
  handles, collider wireframes, light cones, camera frusta are quads and
  lines in that pass). Picking = raycast: physics raycasts exist; for
  non-physics entities, a CPU ray vs entity AABB pass at pick time.
- **Undo/redo:** a command stack of `(path, old, new)` `put()` records over
  the live `Var` tree, coalescing per drag/frame. Because scenes are plain
  data, undo is not special — it's `set_path` bookkeeping.
- **Edit vs play:** edit mode drives `update(0)`/`draw()` on demand (a
  frozen world you can still manipulate — "edit live" is a hyg differentiator
  worth keeping weird); play mode runs the real pump with fixed-dt
  determinism; pause steps one frame at a time. Scene state on entering
  play is snapshotted (deep clone of the `Var` tree — scenes are small) and
  restored on exit, so play experiments never dirty the file.

### Canvas scaling — non-standard aspect ratios

The scene spec is 16:9; surfaces are anything (21:9 ultrawide, 32:9
super-ultrawide, 16:10, portrait phones). The window layer carries a
three-mode **canvas** setting (per project, `project.json.hyg`; Godot's
stretch-mode thinking with hyg names):

- **Fit** — render the 16:9 canvas centered, letterbox bars on the sides,
  by choice. This is today's streaming-window behavior (bilinear upscale,
  letterbox math recomputed on rotation/resize) promoted to a first-class
  mode for every window backend. GUI untouched; bars come back on demand.
- **Expand** — the ultrawide payoff: the **scene fills the real
  resolution** and the camera adapts (perspective aspect = the real ratio;
  ortho half-extents widen by the ratio — more world visible, not bigger
  pixels), while the **UI layer constrains itself to the centered 16:9
  "UI zone"** — a derived safe-area rect. Non-standard ratios need no GUI
  re-anchoring or rescaling: the app presents like a 16:9 window sitting
  in the middle, and the world extends honestly on both sides instead of
  drawing black bars over real rendered scene.
- **Anchor** — the canvas stays 16:9-spec'd (camera untouched, identical
  world framing on every screen) and the **GUI re-anchors/rescales** to
  the real resolution around it. For when the framing *is* the product
  (fixed-camera game) and chrome should use the whole screen.

Design notes: the **UI zone** is a derived `Rect` (16:9 fit inside the
surface, centered) that the UI layer anchors into — it's the contract that
makes Expand safe, and pixel tests pin the zone math at 21:9 / 32:9 / 16:10
before any UI exists. Expand's ortho widening needs a project choice:
`show_more_x` (width ratio scales half-extents.x — wider view) or `zoom`
(height ratio scales it — same width, taller view). The streaming window's
fit/bilinear implementation becomes Fit's engine-side implementation; its
rotation-aware refit is the pattern Expand's ortho path must match. Anchor
is the only mode that needs a separate UI pass on top of the canvas.

### Scripting UX

The v12 seam is the authoring layer: attach a `ScriptedComponent`, edit
`scripts/patroller.js` in a pane, save = reload (a fresh `Script` with the
same id; the thread-local context pool rematerializes — the pool keyed by
script id is the reload seam). The `transform` global the seam exposes is
what the inspector's transform row shows — script and inspector write the
same shared `Arc<Mutex<Transform>>`, so scripted motion is visible in both.

### Asset pipeline

glTF import exists (`gltf` feature: node transforms + material base colors
baked into vertices). The editor wraps it: drag a `.glb` into Assets →
imported into `assets/`, a `mesh_renderer` component draft on the dragged
entity, textures decoded via the `image` feature, audio via the audio
backend's `loadSound`-shaped API. Import is pure data in → data out, so it
runs in headless tests.

## Milestones (proposed)

| Stage | Deliverable | Done when |
| --- | --- | --- |
| M0 | Scene model | Scenes (de)serialize headless; `FromVar` rejects bad scenes with path errors; round-trip test: build → save → load → save is byte-identical |
| M1 | Editor core | winit shell + egui panels + viewport presenting `FrameSink`; hierarchy + inspector editing a live `Engine`; headless UI-logic tests |
| M2 | Manipulation | Selection (picking), transform gizmos, undo/redo, scene save/load from the UI; pixel tests assert gizmo rendering |
| M3 | Play mode | Play/pause/step with snapshot-restore; script editing + live reload; console shows script errors |
| M4 | Assets | glTF/texture/audio import flow; physics + audio debug views (colliders, raycasts, sound list) |
| M5 | Polish | Multi-viewport (camera preview), scene tabs, project settings UI, packaging: `hyg build` emits a runtime binary + data bundle |

M0/M1 are the honest test of the thesis: if the scene model and the shell
feel heavy, the design bends there before anything else.

## Open questions

- **Component schema versioning** — scenes outlive code; descriptors need a
  version field + migration story (a migration is a `Var` → `Var` function
  registered per component type).
- **egui vs the quad pipeline for UI** — egui for panels is settled; whether
  in-viewport overlays (gizmo labels, handles) also go through egui or stay
  in the overlay pass needs a prototype.
- **winit 0.30 + egui integration** on the same surface as the viewport —
  verify layering before M1.
- **Windowing direction — Wayland first, X11 second, HDR on Wayland.**
  winit already prefers Wayland at runtime when a compositor is present;
  keep that ordering and build **HDR** on it — HDR needs the compositor's
  color-management protocols, which makes Wayland the only real Linux path
  (X11 is a dead end there). The seam is wgpu's surface configuration
  (`color_space` beyond the default Srgb) plus the camera/tonemap story
  for HDR values in shaders.
- **Platform matrix** — the editor inherits the engine's platform support;
  see [platforms.md](platforms.md) (Linux suite-verified, Windows
  compile-verified, macOS expected).
