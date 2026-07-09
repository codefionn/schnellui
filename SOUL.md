# SOUL.md — schnellui

> **schnellui** *(schnell = "fast")* — a **macro-first, GPU-first, signal-first** Rust GUI framework.
>
> This file is the *soul* of the project: the load-bearing beliefs, the non-negotiable covenants,
> and the technical north star every contributor — human or AI — reads before touching code.
> If a change contradicts this document, either the change is wrong or this document is out of date.
> Change one of the two, never neither.

---

## 0. One sentence

A retained, GPU-resident UI where **you write UI as macros**, **state flows through signals**, and **a signal changing repaints the screen without allocating a single byte on the heap** — literal zero on the steady-state re-render path, small-and-budgeted everywhere else (§4).

---

## 1. Prime Directives (the covenant)

These are ranked. When two directives conflict, the lower number wins.

1. **Zero allocations on re-render.** When a signal changes and the frame is re-rendered, the steady-state hot path performs **0 heap allocations, 0 reallocations, 0 frees** (`allocs + reallocs + frees == 0`). This is *tested*, in CI, as a hard gate — not an aspiration. First-mount and grow-events may allocate; steady-state re-render may not.
2. **GPU does the drawing.** All 2D content (paths, text, gradients, images, clips, blends) is rasterized on the GPU. The CPU's job is to decide *what changed* and upload *only that*, in bytes, not buffers.
3. **A signal change costs work proportional to what changed** — not to the size of the tree, the screen, or the scene. One text node changing must not walk the whole widget tree, re-tessellate the window, or re-upload the scene.
4. **Macros generate static structure; only dynamic sites get reactive wiring.** The view macro splits UI into an invariant skeleton (hoisted to constants) and a handful of dynamic slots. Work is proportional to the number of dynamic sites, never the total node count.
5. **The framework is legible to an AI agent.** Every example can launch into a named state, render one frame, write a PNG *and* dump its semantic (accessibility) tree, and exit. An agent can enumerate states, screenshot them, query what they *are*, edit, and re-shoot — a closed self-improvement loop with no human in the frame.
6. **Accessibility is a first-class output, not an overlay.** Every widget is *semantic before it is visual*: it carries an AccessKit role, name, value, and state as part of its definition. The accessibility tree is built from the retained tree and updated through the **same signal→damage loop** that drives pixels — a state change emits an incremental `TreeUpdate` of only the changed nodes. A screen reader operates the *real* UI through the same handlers as a mouse. There is no separate, degraded a11y path.
7. **Multithreading is designed in, not bolted on.** The signal runtime does not rely on hidden thread-local global stores in a way that forecloses `Send` rendering. This is the single mistake we refuse to inherit from the JS signal lineage.

> **The test that defines us:** `cargo test --features count-allocations` contains a test named
> `rerender_on_one_signal_allocates_nothing`. If it ever asserts a number `> 0`, the build is red.
> That test is the soul made executable.

---

## 2. Why this exists (the gap we exploit)

The research is unambiguous about where the state of the art leaves room:

- **egui** is pure immediate mode: the whole UI closure re-runs every frame, `epaint::Tessellator` rebuilds `Mesh { Vec<Vertex>, Vec<u32> }` every frame, and fresh GPU vertex/index buffers are re-uploaded every repaint. It is *architecturally* incapable of zero-alloc re-render. We treat it as the anti-pattern.
- **Vello** — the best GPU 2D renderer in Rust — **re-encodes and re-uploads the entire scene every frame**. Verified from source (`resolver.resolve(...)` → `recording.upload("vello.scene", packed)` → `free_resource(scene_buf)`): no persistent GPU scene buffer, no diff, no partial upload, no damage. Retained fragments are aspirational in its own `vision.md`; damage regions are an open issue (`xilem#789`). Its maintainer's own advice is "compare inputs to your paint functions yourself." *That is the gap.*
- **Xilem/Masonry** rebuilds a view tree every cycle and diffs it (React-shaped, not signals), and **explicitly rejects signals** — citing thread-local signal stores as a multithreading impedance mismatch. Masonry caches per-widget scene fragments but still reassembles and re-uploads the whole window scene every paint.
- **Floem** is the closest prior art: retained tree built once + Leptos-style fine-grained signals, no VDOM, no diff. It proves signal-first works — but it is desktop-only and does not chase zero-alloc GPU uploads.

**schnellui's bet:** combine Floem's signal-first retained tree with WebRender's proven "little change → little work" GPU levers (interned retained scene, tile/picture cache with per-tile dependency fingerprints, an indirection GPU cache, incremental `write_buffer`/`write_texture` uploads) — levers Vello deliberately does not use. Then hold the whole thing to an allocation budget enforced in CI.

---

## 3. The three pillars

### 3.1 Signal-first

The reactive core is a **push-then-pull, lazily-evaluated, mark-and-sweep coloring graph** — the "Reactively" algorithm (Milo Fultz), the same family Leptos, SolidJS, and Sycamore converged on, because it is glitch-free on diamonds *and* does the minimum recomputation.

**Node coloring (packed into one byte of bitflags, never several bools):**

| Color | Meaning |
|---|---|
| `Clean` | value valid, nothing to do |
| `Check` | a *transitive* ancestor changed; must verify sources before trusting the cache |
| `Dirty` | a *direct* source changed; must recompute |

**Two-phase propagation:**

- **PUSH (on `set()`)** — mark *direct* observers `Dirty`, mark *deeper* descendants `Check`. **No recomputation.** Notification only; effects are *queued*, not run.
- **PULL (on read / at frame boundary)** — `update_if_necessary()`: `Clean` → return cache; `Check` → recurse into sources *in read order*, stop the instant one resolves `Dirty`; `Dirty` → recompute, run the equality gate, and only if the value *actually changed* re-mark observers `Dirty`. The recursive source-first pull *is* an implicit demand-driven topological order — no explicit sort, glitch-free diamonds, each node recomputed at most once.

**Storage — a single generational arena, not an `Rc` graph:**

```rust
// One process-wide (or per-runtime) slotmap. Handles are Copy integers.
new_key_type! { pub struct NodeId; }
struct Runtime { nodes: SlotMap<NodeId, Node>, /* global_version, queues, current_observers */ }

struct Node {
    value:        Option<Box<dyn Any + Send + Sync>>,   // type-erased cell
    compute:      Option<Box<dyn FnMut(&mut dyn Any) -> bool /*changed*/ + Send>>,
    sources:      SmallVec<[NodeId; 1]>,   // inline the common single-dep case (Sycamore's trick)
    observers:    SmallVec<[NodeId; 2]>,
    version:      u64,                     // bumped only on a real value change
    flags:        NodeFlags,               // Clean/Check/Dirty | Tracking | Disposed | Running
    equals:       Option<fn(&dyn Any, &dyn Any) -> bool>, // OPT-IN equality gate; None = compute decides
}
```

Handles (`Signal<T>`, `Memo<T>`) are `Copy` and carry only a `NodeId` — no `T: Copy` bound, no clone, no refcount bump on read. A parallel `Arc`-backed variant (`ArcSignal<T>`) exists for signals that escape into collections or outlive their owner, mirroring Leptos's proven dual API. Owner tree disposal removes a scope's `NodeId`s from the arena; disposed access returns `None` via `try_*` (we prefer this over panicking).

**Allocation discipline in the graph (this is where zero-alloc is won or lost):**

- **Dependency tracking allocates nothing in steady state.** Edge lists are stable per-node `SmallVec`s. Before a recompute, mark all edges with a `version = -1` back-buffer sentinel; on re-access reuse the marked slot instead of allocating; sweep untouched edges at the end (Preact/Vue technique). Same deps every run ⇒ zero alloc, zero free.
- **A single monotonic `global_version: u64`** gives an O(1) "did anything change anywhere" gate before any graph walk (Preact/Angular/Solid/Glimmer all do this).
- **Change detection has one owner: `compute` returns `changed`.** `version` bumps only on a real change, so a memo boundary stops propagation without re-running its children. The `equals` comparison is **opt-in** (`{ equals: PartialEq }` or a custom fn), never the default. On a signal `set()` it is free — compare the incoming value against the cell before overwriting. On a memo it is not: the compute writes in place, so opting in makes that node retain its previous value (double-buffered — a cost paid once at node *creation*, never on the steady-state path). Without `equals`, `set()` always fires and a memo fires when its compute says so. (Footgun to document loudly: `NaN` under `PartialEq` never equals itself, so an opted-in `NaN`-valued signal re-fires forever — offer a bitwise comparator.)
- **Borrow-safety rules (Rust-specific, non-negotiable):** drop the value guard *before* notifying subscribers; `mem::take` / drain the observer list before iterating it; never run an effect synchronously mid-write — queue it and let the pull phase re-check. Ignoring these reproduces the `already borrowed: BorrowMutError` / "invalid SlotMap key" panics that bit Sycamore and Leptos. Under the arena lock the same rules wear a second face: **never hold the lock while running user code** — computes and effects read other signals and re-enter the runtime. Discipline: acquire → take what the closure needs (`mem::take` the compute fn, copy inputs) → **release** → run → re-acquire to write back; the recursive pull re-acquires per node, never recurses while holding. A lock held across user code is the deadlock twin of `BorrowMutError`.
- **Threading decision, made once, up front:** the default runtime is `Send + Sync` (value cells behind the arena lock; `!Send` values opt into a `SendWrapper` local variant). We accept a small single-threaded tax to keep the door open for multithreaded rendering that Xilem gave up signals to get.

*Prior art to keep studying: Alien Signals (current js-reactivity-benchmark leader) and Signia's epoch + `getDiffSince()` incremental-diff model, for when we have large derived collections.*

### 3.2 GPU-first

**Retained, GPU-resident scene. The CPU uploads deltas, in bytes.**

The renderer is not Vello-as-is. It is Vello's *compute rasterization ideas* (or `vello_hybrid`'s sparse strips for the WebGL2 path) wrapped in a **retained, incrementally-uploaded scene** modeled on WebRender — the reference for GPU-driven 2D that actually does "small change → small work":

1. **Retained scene via content-addressed interning + epoch GC** (WebRender `Interner` + `DataStore`), *not* full-tree diff. Unchanged primitives are never re-hashed, re-uploaded, or rebuilt.
2. **Tile / picture cache — the primary "little change → little work" lever.** Split the scene into slices by *update frequency* (fast-moving UI vs static content) so an animation doesn't invalidate everything; tile each slice; give each tile a **dependency fingerprint** (`TileDescriptor`); the union of invalidated tiles is the frame's dirty rect, used as a scissor + OS partial-present.
3. **Property/transform trees** (Chromium `cc`): scroll, translate, and opacity changes **mutate a property-tree node and re-composite** — no re-raster, no relayout, no repaint. This is how a scroll costs almost nothing.
4. **An indirection GPU cache** (WebRender `GpuCache`): a resident texture of shader params, compact per-instance `ivec4` of packed IDs on the vertex stream, params fetched in-shader by ID. Per-frame we upload **only new/changed blocks** (epoch-validated). **Order params by update frequency** — static matrices at the buffer start, animated ones at the end.
5. **Incremental uploads are the whole point.** The wgpu primitives Vello declines to use are our bread and butter:
   - `Queue::write_buffer` / `write_buffer_with` — patch a few bytes into a resident buffer at an offset (this *is* "one signal → upload a few bytes"; `write_buffer_with` hands you the staging bytes so you skip the intermediate `Vec<Vertex>`).
   - `write_texture` with `TexelCopyBufferLayout` — sub-rect glyph/atlas updates without full re-upload.
   - `draw_indirect` / `multi_draw_indirect` — draw params live on GPU; the CPU touches only counts.
   - Render bundles — replay pre-recorded command sequences for static layers without CPU re-encoding.
   - A `StagingBelt` for the many-small-writes case; grow-only (never-shrink) vertex/index buffers; per-node cached geometry re-tessellated only when *that node's* signal changed.

**The signal → damage loop (this is the core loop of the entire framework):**

```
signal.set(x)
  → runtime marks dependent effect Dirty (push)
  → at frame boundary, pull runs that one effect
  → effect writes ONE property on ONE retained node
  → node marks itself paint-dirty; its paint_rect joins the frame damage region
  → only that region's tiles / GPU-cache blocks re-upload (write_buffer / write_texture)
  → GPU re-rasterizes only the dirty tiles; compositor presents only the damaged rect
```

This skips even the tree diff that GTK, Blink, and Xilem pay. Nothing on the clean path allocates, walks, or uploads.

### 3.3 Macro-first

UI is written as a macro. The macro is not sugar over a runtime interpreter — it **compiles to a statically-typed builder chain** with a compile-time static/dynamic split (Leptos `is_inert_element`, Sycamore `is_dyn`, Dioxus `static Template`).

```rust
view! {
    column {
        text(class = "title") { "Counter" }        // fully static → hoisted to a const, built once
        text { (count) }                            // dynamic slot → wrapped in a RenderEffect
        button(on:click = move |_| set_count(count.get() + 1)) { "increment" }
    }
}
```

- **Static subtrees** — no signals, no dynamic attrs — compile to a single hoisted constant / pre-built node and are never traversed on update.
- **Dynamic sites** — a bare signal `(count)` or a `move || …` closure — compile to a `RenderEffect`-equivalent: it runs synchronously on first render (creating the node), **carries its previous value**, and on every later run **mutates the existing retained node in place** and cancels when its handle drops. Dependency tracking is automatic (the runtime's `current_observer` field — a runtime field, **never a thread-local**, per Directive #7; edges are sentinel-marked and swept per run — §3.1's slot-reuse discipline, zero alloc when deps repeat — so conditional branches only subscribe to what they actually read).
- **The component function is a setup function, not a render function — it runs once.** Reactivity lives only in the leaf closures the macro wraps. This is what makes updates fine-grained instead of re-rendering components.
- **Structure:** the parser/AST lives in a **separate crate** from the proc-macro (`schnellui-view-parser`), exactly as Dioxus (`dioxus-rsx`) and Sycamore (`sycamore-view-parser`) do — so autoformat, hot-reload reparse, and tooling don't re-invoke the compiler. Codegen targets a typed builder chain via a dedicated `Codegen` struct (so it can thread render-mode), not a blanket `ToTokens`.
- **Hot-reload is data, not code.** Emit path-addressable template metadata with stable IDs so static text/attr/child edits ship a new template over a socket with no recompile (dynamic expressions and new closures still need a rebuild — say so honestly).

> **Note the deliberate coupling:** Floem proves signal-first does *not* require a macro, and Xilem proves GPU-first does *not* require one. schnellui chooses macros anyway — for the static/dynamic split that makes "work ∝ dynamic sites" automatic and for hot-reloadable templates. The macro *earns* its place by producing the zero-alloc wiring; it is not decoration.

---

## 4. The allocation covenant (how we keep the promise)

Zero-alloc is a claim only if it is measured. It is measured three ways, and all three run in CI.

### 4.1 Budget tests (exactness)

`tests/alloc_budget.rs` asserts exact allocation counts on named hot paths using the **`allocation-counter`** crate (closure-scoped, feature-gated so normal builds are untouched):

```rust
#[test]
fn rerender_on_one_signal_allocates_nothing() {
    let mut app = App::mount(counter_scenario());
    app.frame();                         // warm caches & pools (first mount is allowed to allocate)
    let info = allocation_counter::measure(|| {
        app.set_signal("count", 42);
        app.frame();                     // steady-state re-render
    });
    assert_eq!(info.count_total, 0);     // allocs
    assert_eq!(info.bytes_total, 0);     // and bytes, and — gate reallocs too:
    // (custom counter wraps realloc: allocs + reallocs + frees == 0)
}
```

Budgets live in one reviewed table (`ALLOC_BUDGETS`), one row per path: `mount`, `rerender_1_signal`, `rerender_n_signals`, `scroll`, `resize`, `text_edit`. **Not every row is zero.** Literal zero is the law on the re-render rows (`rerender_1_signal`, `rerender_n_signals`, `scroll`); the rest are *small, fixed, reviewed* numbers: `mount` and `resize` are grow events by definition, and `text_edit` covers what text honestly costs — shaping runs through a pooled Parley `LayoutContext`/`Layout` (amortized zero once warm), but new-glyph atlas inserts and a11y `TreeUpdate`s (§6.2) may allocate. Raising any budget is a reviewed diff with a justification, never a drive-by. **We gate `allocs + reallocs`, not just `allocs`** — a growing `Vec` shows up as a realloc and still violates the covenant. **We always measure the *second* invocation**, never the first, so a warm-capacity false-pass can't sneak through.

### 4.2 Regression backtraces

When a budget test fails, **`dhat`** (`.testing()` profiler) reports the *backtrace of the allocation that broke the budget* — regressions are self-diagnosing, not a mystery number.

### 4.3 Deterministic CI gate

**`iai-callgrind`** (Valgrind/DHAT-based) provides machine-independent, non-flaky allocation *and* instruction counts, with configurable regression thresholds that fail the run — the right tool for a shared CI runner where wall-clock benches flake. Micro-benches use **`divan`** with its `AllocProfiler` so alloc counts show up as a column alongside timing in every bench run.

### 4.4 The techniques that make it possible

| Concern | Choice |
|---|---|
| Per-frame transient scratch | `bumpalo` `Bump`, `reset()` at frame end (amortized zero heap after warmup; chunks retained) |
| Retained node storage | `slotmap` (primary tree) + `SecondaryMap` columns (layout / paint / bindings split ECS-style) |
| Small child / class / event lists | `smallvec` (spills), `arrayvec` (hard cap, never heaps), `tinyvec` (zero-unsafe where wanted) |
| Cross-frame buffers | long-lived scratch `Vec`s on the app/renderer struct, `clear()` + refill (retains capacity) |
| Widget kind dispatch | enum dispatch, **not** `Box<dyn>` churn; trait objects (if any) allocated once, mutated in place |
| Text / ids | `Cow<'static, str>` labels; `lasso`/`ustr` interning for ids & classes (`Copy` integer handles) |
| Immutable state snapshots | `imbl` / `im-rc` (O(1) structural-sharing clones; unchanged subtrees are the same allocation) |
| GPU buffers | persistent + `StagingBelt` + `write_buffer_with`; grow-only, never shrink |

---

## 5. Backends

Two rendering families, one scene, selected at runtime by capability.

- **WebGPU (native + browser) — the GPU-first target.** Native maps through wgpu to Vulkan / Metal / DX12; browser is a thin shim forwarding to `navigator.gpu` (compute shaders available in-browser since the all-major-browser WebGPU rollout of late 2025). Full compute-rasterization path.
- **WebGL2 fallback — the reach target.** For browsers/devices without WebGPU, use `vello_hybrid`'s `webgl` feature (fragment-shader-only sparse strips, **no compute required**) or wgpu's own `webgl` feature (Naga translates WGSL→GLSL ES at runtime). Sparse strips are the bridge: CPU does everything up to and including coarse raster; the GPU fine-rasters each strip as two triangles.
- **Browser backend.** WASM + wgpu + winit: attach a `<canvas>` via `WindowAttributesExtWebSys::with_canvas`, surface via `SurfaceTarget::Canvas` (or `OffscreenCanvas` to render on a Web Worker off the main thread). `request_adapter` / `request_device` are async.

**Canvas means we own accessibility — so we make it first-class, not a fallback.** Rendering to a GPU canvas means the platform gives us no a11y, text selection, IME, hit-testing, or scrolling for free. We treat that not as a tax to minimize but as a mandate: schnellui ships a full **AccessKit** tree, keyboard navigation, focus, and IME from commit one — see **§6**. An optional DOM backend remains available for content where native browser semantics are non-negotiable.

---

## 6. Accessibility is first-class (AccessKit)

Directive #6 in full. Accessibility is not a compliance checkbox bolted to a finished UI — it is a **second rendering target** that shares the retained tree, the signal graph, and the incremental-update discipline with the pixels. If pixels are one projection of the retained tree, the AccessKit tree is the other, and both are held to the same "little change → little work" standard.

### 6.1 Semantic-first widgets

Every primitive in `schnellui-widgets` declares its semantics as part of its definition, not as an afterthought:

- an AccessKit **`Role`** (`Button`, `TextInput`, `CheckBox`, `Slider`, `List`, `Label`, …),
- an accessible **name** (from label text or an explicit override),
- **value / state** (checked, disabled, expanded, selected, min/max/now for ranges),
- the **actions** it supports (`Click`, `Focus`, `SetValue`, `Increment`/`Decrement`, `ScrollIntoView`).

**The covenant:** no widget ships without a role. A "custom-drawn" widget that paints pixels but exposes no semantics fails review the same way an allocating re-render does.

### 6.2 The a11y tree rides the signal→damage loop

The AccessKit tree mirrors the retained tree one-to-one: each retained node's `NodeId` *is* its AccessKit `NodeId`. It is built once, then updated **incrementally through the exact same reactive mechanism as paint**:

```
signal.set(x)
  → dependent effect writes ONE property on ONE retained node
  → if that property is semantic (name/value/state/focus), the node joins the a11y-dirty set
  → at frame boundary we push accesskit::TreeUpdate { nodes: <only changed>, tree, focus }
  → the platform adapter tells the screen reader exactly what changed — nothing else
```

No full a11y-tree rebuild per frame (the mistake that mirrors Vello's full-scene re-upload). A signal that changes only a label's text produces a one-node `TreeUpdate` and a one-tile repaint — symmetric by design. **Allocation honesty:** `accesskit::TreeUpdate` / `Node` are foreign types that own their storage (`Vec`s, `String`s) — even a one-node update allocates a little, and we cannot pool inside types we don't control. Accepted: the *literal-zero* gate (§4) is the paint re-render path; the a11y-dirty path gets its own small fixed row in `ALLOC_BUDGETS` — proportional to changed nodes, never to tree size — and shrinks further if AccessKit ever exposes reusable buffers.

### 6.3 Actions flow inbound to the same handlers

AccessKit is bidirectional. The platform sends `ActionRequest`s (a screen reader clicking a button, a switch device setting focus, an assistive tool calling `SetValue`), and schnellui routes each one to the **identical handler as the equivalent pointer/keyboard event**:

```
screen reader "clicks" Button  →  accesskit::ActionRequest { action: Click, target }
                               →  same on:click closure a mouse would have fired
```

This is *why* canvas accessibility can be **equal, not degraded**: the assistive path and the pointer path converge on one code path. Focus, tab order (derived from the tree), and IME composition (surfaced into the retained text nodes) are first-class citizens the framework owns end-to-end, because the canvas gives us none of them for free.

### 6.4 Platform adapters

`accesskit_winit` selects the right native adapter automatically: **UI Automation** (Windows), **NSAccessibility** (macOS), **AT-SPI over D-Bus** (Linux), plus **Android**. On the **web/canvas backend**, the browser exposes no native semantics for a canvas, so we maintain a hidden **ARIA/DOM mirror** kept in sync by the same `TreeUpdate` stream. That mirror is real, owned work — we do not pretend it is free (see §11).

### 6.5 Accessibility is a testing and agent superpower

The AccessKit tree is a **queryable semantic snapshot** — and this is where first-class a11y pays for itself twice. The Rust testing harnesses `kittest` / `egui_kittest` are already built *on* AccessKit; we adopt the same idea natively:

- Snapshot tests assert on the **a11y tree**, not just the PNG. "Is there a `Button` named *increment*, enabled, in tab position 3?" is a structural assertion that survives cosmetic pixel churn.
- The screenshotter harness (§7) exposes **`--dump-a11y <path.json>`** so an AI agent queries *semantics* alongside pixels. The screenshot tells the agent **how the UI looks**; the a11y tree tells it **what the UI is** — role, name, state, focus, reading order. An agent that can read both iterates far more reliably than one scraping pixels, and it validates accessibility as a side effect of every screenshot it takes.

Accessibility, in other words, is not only the right thing to do — it is the machine-readable ground truth that makes Directive #5's self-improvement loop robust.

---

## 7. Screenshotter examples (the AI-agent self-improvement loop)

**Every example is a one-shot screenshotter.** This is a first-class pillar, not a testing afterthought — it is how an AI agent (or a human) sees the UI without a human describing it.

### 7.1 The CLI contract

```
schnellui-example --scenario <name> --out <path.png>
                  [--width W --height H --scale S --seed N --theme dark]
                  [--list] [--all --out-dir DIR] [--manifest manifest.json]
                  [--dump-a11y <path.json>] [--assert]
```

Each example: build the GPU context once → put the scenario's UI in a **specific state** (constructed directly, *or driven there through AccessKit actions* — §7.5) → render exactly **one synchronous frame** → `copy_texture_to_buffer` → read back → encode PNG → `std::process::exit(0)`. **No event loop.**

- `--list` prints scenario names one per line (exit 0) so an agent can *discover* states.
- `--all --out-dir DIR` renders every scenario to `DIR/<name>.png`.
- `--manifest` emits `[{scenario, path, width, height}]` so the agent maps names → files without scraping.
- `--dump-a11y <path.json>` writes the full AccessKit tree (roles, names, values, states, focus, reading order) alongside the PNG — the machine-readable ground truth of §6.5, so an agent (and snapshot tests) can query *what the UI is*, not just how it looks.
- `--assert` runs the scenario's embedded AccessKit assertions (role / name / value / state / focus) and exits nonzero on failure — the a11y tree is the **primary correctness oracle**, the PNG the secondary visual check (§7.5).
- Scenarios are registered in an enumerable table (`clap::ValueEnum` + `strum::EnumIter`) so `--scenario` is validated and the set is introspectable.

### 7.2 Headless render → PNG (the exact path)

Offscreen texture (`RENDER_ATTACHMENT | COPY_SRC`, `Rgba8UnormSrgb`) → render pass → `copy_texture_to_buffer` into a `COPY_DST | MAP_READ` buffer → `map_async` (callback-based; **must `device.poll(PollType::wait_indefinitely())`** before the callback fires) → `get_mapped_range` → encode with the `png`/`image` crate.

> **The 256-byte gotcha, written down so nobody rediscovers it:** `copy_texture_to_buffer` requires `bytes_per_row` to be a multiple of `COPY_BYTES_PER_ROW_ALIGNMENT` (256). Compute `padded_bytes_per_row`, and strip the padding per-row when encoding. Always pad — never rely on the width happening to be aligned.

### 7.3 Determinism (so screenshots are stable and diffable)

- Fixed viewport, `scale_factor = 1.0`, fixed clear color, MSAA off or pinned.
- **Logical clock injected as `now = 0`** — no `Instant::now()`, no animation loop, one frame. Every RNG seeded.
- **Embedded font** (`include_bytes!`) — never a system font. Pinned shaper/rasterizer settings; subpixel AA off. (Font AA is the single hardest determinism problem; expect it and tolerate it — see below.)
- **Software adapter selectable** (`--backend software` → lavapipe / SwiftShader / WARP) for cross-machine-stable goldens, mirroring what `egui_kittest` defaults to.

### 7.4 Snapshot diffing & blessing

Goldens live at `snapshots/<scenario>.png` (lossless-optimized with `oxipng`, git-friendly; large/binary assets via Git LFS as Vello does). On mismatch, write `snapshots/<scenario>.diff.png` as a visual artifact the agent can look at. Because text/AA make exact byte-match brittle, **compare with a perceptual tolerance** (`dssim` / `image-compare` SSIM / `dify`), not `==`. Re-bless via an env var (`SCHNELLUI_BLESS=1`), following the `UPDATE_SNAPSHOTS` / `MASONRY_TEST_BLESS` convention.

### 7.5 Driving *and* asserting state through AccessKit

The accessibility tree is not only an *output* to inspect (§6.5) — in the screenshotter it is both the **driver** that reaches a state and the **oracle** that verifies one. This is the same model `kittest` / `egui_kittest` and `iced_test::Simulator` use; we adopt it natively.

**Two ways to reach a scenario's target state:**

1. **Construct** — build the UI already in that state. Pure, fast, deterministic; best for static appearance checks.
2. **Drive** — start from a base state and dispatch a sequence of AccessKit `ActionRequest`s (`Click`, `Focus`, `SetValue`, `Toggle`, `Increment`/`Decrement`, `ScrollIntoView`) to walk the UI into the target state, exactly as a user or screen reader would. Targets are located by **semantic query — `Role` + accessible name — never by pixel coordinates**, so a drive script survives restyles, theme changes, and layout reflow. Because driving goes through the *same inbound `ActionRequest` path as real input* (§6.3), it exercises the actual event handlers: it proves the state is **reachable**, not merely constructible.

**Assert against the a11y tree (the oracle).** After building or driving, assert on role / name / value / state / focus / reading order as the **primary** correctness check; the PNG is the secondary visual check. Semantic assertions survive cosmetic pixel churn that would false-positive a pixel diff, and every assertion doubles as an accessibility audit on that state.

```rust
scenario!("counter_reaches_five")
    .build(|| view! { Counter(start = 0) })
    // find by semantics, actuate through the real ActionRequest path (§6.3):
    .drive(Action::Click, at(Role::Button, name = "increment")).times(5)
    // the a11y tree is the oracle — checked before the shot is even taken:
    .assert(at(Role::Status).value_contains("5"))
    .assert(at(Role::Button, name = "decrement").is_enabled())
    .assert_focus(name = "increment")
    .shoot();   // → PNG + a11y dump, only after the assertions pass
```

**Determinism holds:** each drive step is *synchronous* — dispatch action → settle the signal graph → (optionally) render a frame — with no wall clock (§7.3), so a driven scenario is as reproducible as a constructed one. **On failure**, dump the a11y-tree JSON *and* the PNG *and* the diff, so the agent sees both what the UI **is** and how it **looks** — enough to self-correct without a human.

### 7.6 The agent loop this enables

```
agent: --list                         # discover available states
agent: --all --out-dir /tmp/shots     # render them all, one frame each, native, no window
agent: (vision) inspect each PNG       # "the increment button has no padding"
agent: --dump-a11y /tmp/tree.json       # query semantics: role/name/state/focus/reading order
agent: edit the UI code
agent: (drive by role+name, not pixels) # reach new states via AccessKit actions (§7.5)
agent: --scenario counter --assert      # re-shoot + assert the a11y tree is still correct
agent: dssim compare vs prior/golden    # measure the visual delta
agent: iterate
```

Fast because each run is one synchronous native frame. This is Directive #5 made concrete: **the framework is legible to a machine that can see.**

*Harnesses worth cribbing rather than reinventing: `egui_kittest` (state → render → PNG → dify diff → bless), `iced_test::Simulator` (input simulation, `Snapshot::matches_image`), `masonry_testing::TestHarness` (`assert_render_snapshot!`, Vello+wgpu headless).*

---

## 8. Crate architecture (proposed)

```
schnellui-signal        # the reactive arena: NodeId, Signal, Memo, Effect, push-pull coloring
schnellui-view-parser   # syn/rstml parser + AST (separate, for tooling & hot-reload)
schnellui-macro         # the proc-macro: view! → typed builder chain, static/dynamic split
schnellui-scene         # retained scene: interned primitives, tiles, property trees, GPU cache
schnellui-render-wgpu   # WebGPU/WebGL2 backend: incremental upload, damage, compute/strip raster
schnellui-text          # Parley + Fontique + shaping; glyph atlas (write_texture sub-rects);
                        #   embedded multi-face family (sans ×4 + mono ×2) + per-span rich shaping
schnellui-a11y          # AccessKit: semantic tree from the retained tree, incremental TreeUpdate,
                        #   inbound ActionRequest routing, platform adapters (winit) + web ARIA mirror
schnellui-layout        # geometry ONLY: Taffy (Flex/Grid/Flow) wrapper; row/column/stack/grid/scroll/pad
                        #   — no pixels, no roles, no content input
schnellui-widgets       # content primitives: text/button/checkbox/slider/input/image/icon
                        #   — draws pixels + carries a11y role/state + handles content input;
                        #   rich text: RichDoc model + RichText viewer + TextArea editor
                        #   (format importers are application code — the model is the seam)
schnellui-theme         # ready-made Theme instances; the Theme/Shape abstraction stays in widgets
schnellui               # the umbrella: App, mount, event loop, winit glue
schnellui-testing       # headless harness: scenario table, PNG readback, dssim diff, a11y-tree dump, bless
examples/*              # every example is a one-shot screenshotter + a11y-tree dumper (§7)
```

### 8.1 Two layers: component library vs layout

schnellui keeps two concerns strictly apart that many toolkits blur together. **The component library answers *what* is on screen; layout answers *where* and *how big*.** They compose in the same `view!` tree and look syntactically alike, but the macro classifies each node by kind and they flow through different passes.

| | **Component library** (`schnellui-widgets`) | **Layout** (`schnellui-layout`) |
|---|---|---|
| Answers | *What* is on screen | *Where* / *how big* |
| Node kind | Content / leaf: `text` `button` `checkbox` `slider` `text_input` `image` `icon` | Containers: `row` `column` `stack` `grid` `flex` `spacer` `scroll` `pad` |
| Draws pixels? | **Yes** — emits scene fragments | **No** — emits only rects + transforms |
| Accessibility | **Always** a `Role` + name / value / state / actions | Structural — usually transparent, or a `Group` |
| Content input? | **Yes** — click, key, toggle, text entry | Only geometry: scroll offset, hit-test routing |
| Intrinsic size? | **Yes** — measures itself (text metrics, min size) | **No** — derives size from children + constraints |
| Engine | schnellui retained widgets over the Vello scene | **Taffy** (Flexbox / Grid / Flow) |
| Dirty channel | *paint-dirty* (+ *a11y-dirty*) | *layout-dirty* |

**The layers never bleed.** A `column` cannot draw and has no accessible role of its own; a `button` cannot lay out its siblings. A widget *measures* itself and hands an intrinsic size up to layout; layout *positions* it and hands a rect back down to paint. That one-way contract is the seam.

**Three orthogonal dirty channels — the payoff of the split.** This is *why* "little change → little work" (§1, §3) actually holds. A signal write flags only the channels it touches:

- change a label's **text** but not its box → **paint-dirty only**: re-shape through the pooled Parley context (amortized zero-alloc once warm), re-raster one tile + one-node a11y `TreeUpdate`, **no relayout**. New glyphs are atlas grow-events; the residual is budgeted as `text_edit` (§4.1).
- change something that alters a node's **measured size** (longer text, an added child) → **layout-dirty**: Taffy relayouts the *smallest affected subtree*, which then repaints.
- **scroll / translate / opacity** → *neither*: mutate a **property-tree node** and re-composite (§3.2) — no relayout, no repaint.

(This mirrors Blink's separate layout-invalidation vs paint-invalidation bits — a proven "small change → small work" design, not an invention.)

**Pass order**, each frame, walked only over dirty subtrees:

```
pull signals → layout (if layout-dirty) → compose (transforms) → paint (if paint-dirty) → a11y (if a11y-dirty)
```

Layout results live in their own `SecondaryMap<NodeId, LayoutBox>` column, separate from the paint and a11y columns (the ECS-style split from §4) — so a relayout writes geometry without touching paint caches or signal edges, and a repaint reads geometry without recomputing it.

Text via **Parley + Fontique** with an embeddable font for deterministic shots. Widgets feed Parley's measured text metrics up as their intrinsic size, so layout never needs to know how text is shaped — only how big it came out.

---

## 9. Non-goals & anti-patterns (what schnellui refuses to be)

- **Not immediate mode.** We never re-run the whole UI per frame, never re-tessellate unchanged geometry, never re-upload the window. (egui's model — respected, but rejected for our goal.)
- **Not a full-scene re-uploader.** We never `resolve → upload whole scene → free` per frame. (Vello's current model — the exact gap we exist to close.)
- **Not VDOM diffing.** No virtual tree, no per-frame reconciliation walk. Signals wire directly to the one node they change. (Dioxus/React — a different, valid trade we don't take.)
- **Not `Rc`-graph reactivity.** No `Rc<RefCell<…>>` signal soup; the arena + `Copy` handles + `SmallVec` edges are the point.
- **Not thread-local-locked-in.** We refuse the one design choice (hidden TLS store foreclosing `Send`) that made Xilem abandon signals.
- **Not accessibility-last.** We never ship a widget that draws pixels but exposes no role, and never treat the a11y tree as a post-hoc overlay. Semantics are part of the widget, updated on the same loop as paint.
- **Not "allocations are cheap, ship it."** The whole project is a wager that they aren't, and that a UI can prove it.

---

## 10. Prior art & sources (stand on these shoulders)

- **Signals:** Reactively / `Reactive-algorithms.md` (Milo Fultz) · SolidJS `signal.ts` · Leptos `reactive_graph` (Clean/Check/Dirty, arena `SlotMap`, Copy handles, `ArcRwSignal`) · Sycamore Reactivity v3 · Preact/Vue 3.5 intrusive-DLL + version-diff · Angular epoch graph · Signia epochs · js-reactivity-benchmark (Alien Signals leads).
- **GPU 2D:** Vello (`ARCHITECTURE.md`, `encoding.rs`, `render.rs`) · `vello_hybrid` / `vello_cpu` sparse strips (Stampfl thesis) · WebRender (interning, picture/tile cache, GpuCache, batching) · Chromium `cc`/RenderingNG (property trees, tiling) · GTK4 GSK render-node diff · wlroots damage ring · Raph Levien's GUI/GPU blog trail · Slint software renderer.
- **Allocation discipline:** `allocation-counter` · `dhat` · `stats_alloc` · `iai-callgrind` · `divan` `AllocProfiler` · `bumpalo` · `slotmap` · `smallvec`/`arrayvec`/`tinyvec` · `imbl` · `lasso`/`ustr` · wgpu `StagingBelt`/`write_buffer_with`.
- **Screenshot harness:** wgpu `render_to_texture` example (256-byte align, `png` crate) · `egui_kittest` · `iced_test` · `masonry_testing` · `dssim` / `image-compare` / `dify` / `pixelmatch`.
- **Accessibility:** AccessKit core (`Node` / `Role` / `TreeUpdate` / `ActionRequest`) · platform adapters `accesskit_winit` (UI Automation / NSAccessibility / AT-SPI / Android) · `kittest` / `egui_kittest` (AccessKit-based semantic snapshot queries) · Masonry's AccessKit pass as an integration reference.

---

## 11. Open bets & unsolved problems (honesty section)

We do not pretend these are solved. They are the frontier.

1. **GPU-driven retained scene is genuinely hard.** Raph Levien's own conclusion: fully retained GPU scenes are gated on GPU dynamic allocation that today's APIs lack. Our tile/property-tree/GpuCache approach is the *pragmatic* path around it, not a claim to have solved the deep problem.
2. **Font AA determinism** across machines will never be perfect. We lean on software rasterizers + perceptual diff tolerance rather than chasing byte-exact goldens.
3. **`Send` signal runtime vs single-threaded speed — and lock correctness.** The `reactive-signals` crate shows single-threaded `UnsafeCell` can be ~40% faster / ~20% leaner than a locked graph. We pay some of that to keep multithreaded rendering open. The lock is a *correctness* problem before it is a perf problem: recursive pull plus user closures re-entering the runtime makes the never-hold-across-user-code discipline (§3.1) load-bearing — getting it wrong is a deadlock, not a slowdown. If the tax proves too high, revisit — but never by smuggling in a TLS store that forecloses `Send`.
4. **Web accessibility on a canvas** is the hard remainder — the one place "first-class" still costs real work. Native platforms get a genuine AccessKit adapter (UIA / NSAccessibility / AT-SPI); the browser exposes no native semantics for a canvas, so we maintain a hidden ARIA/DOM mirror synced from the same `TreeUpdate` stream (§6.4). Links, native scroll, find-in-page, and text selection need explicit implementation. First-class means we own it end-to-end, not that it is free.
5. **Zero-alloc under `dyn`.** Type-erased signal cells (`Box<dyn Any>`) allocate at creation. Steady-state re-render must touch none of that — verified by §4, but the boundary is subtle and worth guarding.

---

## 12. How to use this document

- **Before you write code:** read §1. If your change makes a re-render allocate, walks the tree on a single-signal change, or re-uploads the whole scene, stop — you're fighting the soul.
- **Before you review code:** the allocation budget test is the tie-breaker. Numbers, not opinions.
- **When you learn something new:** if it contradicts §2–§11, verify against primary source (the framework's *source*, not its blog), then update this file in the same PR. This document earns its authority by staying true.

*schnell heißt schnell.*
