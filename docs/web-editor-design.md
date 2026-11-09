# Hydrargyros Press — a WYSIWYG website & API developer editor

**Short name: `hyg` Press** (Hydrargyros is the project; `hyg` is the
namespace).

**Status: draft for review — not a commitment.** This sketch describes what a
WordPress/Ghost-style editor for the web module could look like while staying
hyg-native. Nothing here exists yet; milestones are proposals.

## Vision

A web app that edits hyg web projects, served *by* the thing it edits: the
editor is itself a hyg `Server` — it hosts the admin UI, a live preview of
the site under edit, and the project's own routes (the WordPress model: one
process is both CMS and site). Every editing surface is a projection of a
structure the web module already ships:

| Editor surface | The hyg structure it edits |
| --- | --- |
| Pages (WYSIWYG) | `web::html` builder trees — elements are data (`tag`/`attr`/`items`), the parser round-trips them |
| Themes | `web::css` parsed rules — typed declarations, `name!` flags; edit structured or raw |
| Content | `web::store` collections through the `database` facade — `$set`/`$unset`/`$inc`, `$in` queries, Mongo-shaped ids |
| Routes & API | the `Server` route table — `Var -> Var` handlers; scripted handlers via the v12 seam |
| Files & uploads | static `mount`s with mime detection |
| TLS & domains | the `sslCert`/`sslKey` config pair |

The WYSIWYG is honest about the representation: what you edit **is** the
builder tree, not a parallel rich-text document that gets converted (the
round-trip parser means serialization is lossless, so "save" is
serialize-the-tree, and hand-edited templates survive WYSIWYG edits).

## Non-goals (v1)

- No multi-user real-time collaboration (single admin + token, later roles).
- No marketplace/publishing pipeline (that's a later release) — but the
  **plugin seam arrives through the `dynamic` module**
  ([dynamic-design.md](dynamic-design.md)): native FFI and sandboxed
  graph/WASM plugins load, hot-reload on a live server, and every artifact
  a future registry would index already has its shape.
- No newsletters/email campaigns (Ghost territory).
- No visual theme *drag-and-drop layout engine* — themes are CSS; the
  editor makes CSS structured, not magic.

## Architecture

### Project model

```
my_site/
  site.json.hyg       # server config (host/port, sslCert/sslKey), theme
                      # selection, route table, admin token hash
  content/            # store collections: posts, pages, tags, media meta
  themes/
    default/
      theme.css       # parsed by web::css
      templates/      # builder trees (serialized Var): layout, post, index
  routes/             # v12 script sources for custom API handlers
  assets/             # uploaded media (static mount)
```

`site.json.hyg` is the single source of truth (a `.hyg` carrier file — the
serde-format-suffix convention shared with Hydrargyros Studio,
[game-editor-design.md](game-editor-design.md)); everything else is content
the editor reads and writes through the module's own APIs. A project
directory is a deployable artifact: `hyg serve my_site/` with no editor
attached runs the site alone (the editor is a mode, not a dependency).

### The editor server

One `Server`, three route groups:

- `/admin` — the editing UI. Server-rendered through the HTML builder
  (dogfooding; no JS framework dependency in v1) with islands of vanilla JS
  where interactivity earns it (the WYSIWYG canvas, the route test
  console). Admin access gates on a token — the streaming window's
  `.with_token()` pattern (random hex, hashed at rest) plus a session
  record in a store collection.
- `/api/*` — the content API: CRUD over store collections, media upload,
  theme/route file operations, and **editor control** (reload theme,
  restart server with new `site.json.hyg`). This is also the programmatic API —
  a headless client (CI, an AI agent) can build and publish a site without
  the UI. The whole request/response surface is `Var`, so any scripting
  client talks to it natively.
- `/preview` — renders the site under edit through the theme pipeline, with
  draft (unpublished) content visible. Live reload rides the streaming
  precedent: `Response::stream` pushes a rebuild event on save, the preview
  page re-fetches. Edit on a laptop, watch a phone on the LAN — the
  streaming window already proved the QR/LAN hand-off trick.

### WYSIWYG page editing

- **Block model:** a page is a list of blocks; a block is one builder
  element (or a small fixed pattern: section+container+children). The
  palette offers the builder's vocabulary — headings, text, images, lists,
  links, code, embeds — not an arbitrary HTML free-for-all.
- **Editing mechanics:** `contentEditable` on the live preview node; on
  blur/debounce, diff the DOM against the block's builder subtree and apply
  mutations back to the tree (`tag`/`attr`/`items`), then re-serialize.
  Text edits are item mutations; structural ops (move/duplicate/delete
  block) are tree ops from the block toolbar. The invariant: the stored
  tree is always valid builder data — the parser can read it, templates can
  mix with it, tests can assert on it.
- **Templates vs content:** a template is a builder tree with slots
  (`Var` object with named holes); a post is content slotted into one.
  Editing a template edits the theme; editing a post edits content — the
  same WYSIWYG, different write targets.

### Theme editing

CSS is parsed into rules and shown as a structured editor (selector,
declaration rows with typed values, `!important` toggled as the `name!`
flag it already is). Raw text mode coexists — the parser's
malformed-input recovery means a hand-edited file never bricks the theme.
Save = parse + validate + write + preview-reload event.

### API & routes designer

- The route table renders as rows: method, `:param`/`*` pattern, handler
  (native function reference or v12 script). New route → pick method +
  pattern → handler editor opens.
- **Handler scripts** are v12 sources with the JSON seam the game module
  already uses: `Var` request in (`method`/`path`/`params`/`query`/
  `headers`/body), `Var` response out. The editor's script pane shares the
  game editor's UX goal: edit, save, reload without process restart.
- **Test console:** fire real requests at the in-process server (real
  traffic, not mocks — matching the test philosophy), show the staged
  `Var` request and the response with status/headers/body. Save the
  request as a regression snippet.

### Route APIs as event graphs

A route doesn't have to be code — it can be a **graph**. The same visual
programming layer the game module gets
([dynamic-design.md](dynamic-design.md)) renders route handlers as event
graphs: `web/request` → flow nodes (branch on the request, loop over a
collection, call a store query or a plugin function) → `web/respond`. It's
the video-game-dev Blueprint experience applied to an API — design the
request flow visually, because the carrier is the same `.hyg` graph
document, the validators are the same, and **conversion IS validation**
(errors land on the exact node and pin, with the flow chain walked back to
the entry). The route table above stays the index: rows whose handler is a
graph open the canvas instead of the script pane. Auth middleware, webhook
pipelines, and whole API folders are graphs too; the test console drives
real requests through the live graph, breakpoints and all.

### Agentic drafts — designed here, delivered later, security-gated

The `/api` surface already accepts headless clients ("CI, an AI agent") —
an LLM service (local ollama for privacy, zai/deepseek via adapters) is
just such a client, and the agentic transform lands **directly in the
reviewable artifact**: prompt → *graph draft*, skipping prompt → code
entirely. The enduser visually scans a graph — no programming knowledge
needed to judge it — and conversion-as-validation gives the model its own
compile errors as feedback. Generated regression suites seed from saved
test-console requests; database scaffolding seeds from the data
designer's observed shapes. **None of it ships before the security floor**
([dynamic-design.md](dynamic-design.md)): generated artifacts run in the
wasmtime sandbox, credential paths are policy-screened (salted+hashed or
rejected — the host enforces it, the prompt can't argue), and nothing
applies without the enduser's visual scan. The human gate *is* the
product.

### Data designer

Collections listed with field inference from existing documents (the store
is schemaless; the designer shows the *observed* shape, never enforces one
— `$set`/`$unset` keep it honest). Query builder covers `$in` and the
field predicates; anything more complex drops to a JSON query box that
takes the exact `Var` the API takes.

## Milestones (proposed)

| Stage | Deliverable | Done when |
| --- | --- | --- |
| M0 | Project model | `site.json.hyg` + directory layout load/save; a project dir runs standalone under a plain `Server` |
| M1 | Admin shell | `/admin` served from the HTML builder, token auth, `/api` content CRUD over a real store — integration tests drive real HTTP |
| M2 | Content & pages | Post/page editing (structured, not yet WYSIWYG), draft/publish states, template slotting |
| M3 | WYSIWYG | contentEditable block editing with tree round-trip; lossless-edit test: edit → save → load → edit → save is tree-identical |
| M4 | Themes | structured CSS editor + raw mode + preview reload; TLS project serves HTTPS end-to-end |
| M5 | API designer | route table CRUD, v12 handler editing with live reload, test console against in-process traffic |
| M6 | Ship | `hyg serve` (site-only) vs editor mode, media uploads, LAN preview hand-off, docs |

## Open questions

- **Admin UI without a JS build chain** — server-rendered builder HTML +
  vanilla JS islands may hit a ceiling at the WYSIWYG stage; if it does,
  the fallback is a single pre-built bundle mounted as a static asset
  (still no Node in the project's dev loop).
- **Concurrent editor + server state** — `site.json.hyg` edits need the same
  restart discipline the engine's `start()`/`stop()` has (no double-start;
  watch-based stop with grace drain). Probably: editor writes config,
  then performs the existing graceful restart.
- **Content versioning** — store collections keep latest-state documents;
  drafts/publish history may want a revisions collection before M3.
- **Windows/macOS parity** — file watching and path handling for content
  dirs must survive the cross-platform audit (see
  [platforms.md](platforms.md)).
