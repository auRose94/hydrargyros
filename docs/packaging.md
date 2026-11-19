# Packaging — from Studio to single file

Hydrargyros games ship two ways, and both start from the same place: a
saved scene (`.json.hyg`) and whatever it references (scripts,
asset meshes). This guide walks the whole path — the quick jam shape
and the configurable shape.

## The two shapes

| Shape | What it is | Who it's for |
| --- | --- | --- |
| **Single file** (`hyg fuse`) | the runtime binary with your game appended — one file, passed around, double-clicked | game jams, itch.io, "send it to a friend" |
| **Bundle directory** (`hyg build`) | the scene + its files in one folder, run by a runtime binary kept beside it | Steam-style installs, when a storefront wants separated data |

Both shapes load the same references by the same rules, so a project
moves between them without edits.

## The CLI

```console
$ hyg new "Star Sweeper" --out starsweeper
new project:     starsweeper/project.json.hyg
  scene:  starsweeper/scene.json.hyg
  script: starsweeper/scripts/main.js

$ hyg build starsweeper/scene.json.hyg
bundle: starsweeper/scene-bundle/scene.json.hyg
  + starsweeper/scene-bundle/scripts/main.js

$ hyg fuse --runtime target/release/hyg-runtime \
            --scene   starsweeper/scene.json.hyg \
            --out     mygame.bin
fused mygame.bin (22833 KB)
```

- **`hyg new`** scaffolds a playable project: `project.json.hyg`, a
  hello scene, a starter script, and the `scripts/` + `assets/` homes.
- **`hyg build`** gathers the scene and every file it references into
  one directory, preserving relative layout. A missing reference is a
  loud error naming the path.
- **`hyg fuse`** builds the bundle, pads the runtime to a 512-byte
  boundary, appends the bundle as a tar archive, and seals a 16-byte
  footer (`HYGFUSE1` + archive length). The padding means ordinary
  tar tools can read the archive straight off the fused file.

## The runtime

`hyg-runtime <scene.json.hyg>` plays a scene in a window. It reads an
optional `project.json.hyg` beside the scene for the game's name,
version, entry scene, and window shape — a bare scene still runs.

**A fused game needs no arguments at all**: the runtime reads its own
tail, finds the footer, unpacks the archive, and plays the scene it
carries. A bare (unfused) runtime falls back to the usage text.

## Modding: the player owns the game

Loose files **beside the executable** override the fused archive —
same relative paths, union semantics:

```
mygame.bin                 ← the shipped game
scripts/patrol.js          ← a player's mod: replaces the shipped copy
scene.json.hyg             ← a total conversion: replaces the game
```

Extract the shipped files to start from the defaults:

```console
$ hyg-runtime --extract        # beside the exe; existing files are skipped
extracted 3 files to . (existing files are skipped)
```

Extraction is nondestructive by design — re-running after edits never
clobbers them. Delete a loose file to fall back to the shipped copy.

## Export from the Studio

The editor does the whole flow: **File → Export Game…** bundles the
saved scene, finds the runtime binary (`HYG_RUNTIME` env, or
`target/release|debug/hyg-runtime`), and fuses — one game file lands
beside the scene, and the rainbow beam hums green.

## Project document

`project.json.hyg` (beside your scenes) is the configuration
centerpiece — name, version, entry scene, window shape:

```json
{
 "name": "Star Sweeper",
 "version": "0.3.0",
 "entry": "scene.json.hyg",
 "window": { "width": 1280, "height": 720 }
}
```

The runtime announces name + version at launch, opens the window at
the configured shape, and `hyg build` carries the document into every
bundle. The Studio's **File → Project Settings…** edits it.
