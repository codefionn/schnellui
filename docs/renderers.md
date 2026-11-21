# Renderers & Deployment Targets

`schnellui` supports multiple rendering backends and platforms. Choosing the right entry point depends on whether you are building a retained native app or a multi-target `Template` app.

## Two Authoring Paths

1. **Retained Native Path (`View` / `view!`)** — Builds directly into the retained `Scene`, Taffy layout engine, Parley text engine, and AccessKit tree. Renders via:
   - **Native GPU (`render_wgpu`)**
   - **SIMD CPU (`render_soft`)**
   Best for rich desktop apps, terminals, editors, embedded Servo browsers, and zero-allocation steady-state updates.

2. **Renderer-Generic Path (`schnellui::template::Template`)** — Describes a statically typed component tree (nested tuples, no type erasure) consumed by any `TemplateRenderer`. The exact same `Template` value can render to:
   - **Retained Native (`SceneTemplate` → `App::mount_template`)** → GPU or SIMD CPU
   - **Semantic HTML & CSS (`render_html`)** → Browser WASM CSR, SSR, or headless Chromium PNG
   - **Native Android Views (`render_android`)** → Real `android.widget` views via JNI

```rust
use schnellui::template::{column, Button, Text};
use schnellui::App;

fn greeting() -> impl schnellui::template::Template {
    column()
        .gap(8.0)
        .child(Text::new("Hello from Template"))
        .child(Button::new("Continue"))
}

// Target 1: Retained native GPU / CPU scene
let app = App::mount_template(greeting());

// Target 2: Semantic HTML & CSS (with `--features render-html`)
// let doc = schnellui::render_html::HtmlRenderer::new(640, 360).render(greeting());

// Target 3: Native Android op buffer (with `--features render-android`)
// let mut renderer = schnellui::render_android::AndroidRenderer::new(viewport, greeting);
```

---

## 1. Native GPU Renderer (`schnellui::render_wgpu`)

The default native backend targets Vulkan, Metal, and DX12 through `wgpu`.

### How It Stays Fast

- **Retained per-node instance slots:** Rather than rebuilding and re-uploading the entire scene on every frame, `GpuCore` maintains CPU mirrors (`Instances`) and per-node ranges (`FragmentTable`) in the quad, glyph, and image instance buffers. A dirty node patches only its own slot and uploads only the changed byte spans via `Queue::write_buffer` (merging nearby spans within 8 instances to avoid staging overhead).
- **Ranged primitive patches:** Nodes that edit a subset of their primitives via `Scene::paint_range_mut` (such as a single row in `TerminalGrid`) re-encode and upload only those primitives.
- **Scroll child indices:** Large scrollable containers (≥ 16 children) build a segment tree (`ChildIndex`) over child bounds, keeping scroll re-walks proportional to visible rows rather than total rows (5,000 and 50,000 rows take the same ~14 µs per step).
- **128 px tile picture cache:** A persistent GPU picture is divided into 128×128 px tiles. Each tile tracks a dependency fingerprint updated incrementally from `changed_spans`. Only invalidated tile rectangles are scissored, rasterized, and copied into the cached picture.
- **Sub-rect atlas uploads:** Both `GlyphAtlas` (R8) and `ImageAtlas` (RGBA8) log written rectangles so only newly rasterized glyphs or updated video/browser frames upload via `write_texture`.
- **Low frame latency:** Native surfaces request `desired_maximum_frame_latency = 1` so drags and scrolls stay tightly coupled to input.

### Selecting a GPU / Software Adapter

```rust
use schnellui::render_wgpu::Backend;

app.set_backend(Backend::Auto);      // Hardware GPU, falling back to render_soft if unavailable
app.set_backend(Backend::Software);  // wgpu software adapter (lavapipe / SwiftShader / WARP)
app.set_backend(Backend::Cpu);       // Built-in SIMD CPU renderer (no wgpu adapter needed)
```

Environment override: `SCHNELLUI_BACKEND=cpu` or `SCHNELLUI_BACKEND=software`.

---

## 2. SIMD CPU Renderer (`schnellui::render_soft`)

`render_soft` rasterizes the **exact same `Instances` buffers and `TileDamage` dirty rectangles** as `render_wgpu`, directly on the CPU with 8-lane SIMD (`AVX` intrinsics when available, or portable auto-vectorized arrays via `SCHNELLUI_SIMD=portable`).

### Why It Exists

1. **Instant window startup:** Even when a GPU is available, `WindowRenderer` opens the native window immediately on `render_soft` (via `softbuffer`) while initializing `wgpu` on a background worker thread. Once the GPU pipelines are ready, the window seamlessly switches to GPU presentation and drops the software presenter.
2. **Zero-GPU environments:** Headless CI runners, minimal VMs, containers, and remote sessions without Vulkan/Metal/DX12 still render pixels accurately with zero setup.
3. **Pixel parity & zero-alloc:** The software kernels mirror the WGSL shaders (including 1/256 vertex snapping, rounded-rect SDF, clip rects, and sRGB quantization) within ~1 code value, and steady-state CPU re-renders allocate **0 bytes** (gated by `rerender_drawn_on_the_cpu_allocates_nothing`).

---

## 3. Semantic HTML, CSS, WASM & SSR (`schnellui::render_html`)

Enable with Cargo feature `render-html` (and `ssr` for server-side rendering and routing).

Unlike canvas-based web UI frameworks, `render_html` compiles `Template` trees into **real semantic HTML elements and CSS stylesheets** — giving you native browser accessibility, text selection, form submission, find-in-page, and CSS layout out of the box.

### Key Capabilities

- **Single-buffer HTML emission:** `DomTemplate` writes markup into a single `String` buffer and tracks node ranges without intermediate string splicing.
- **Responsive CSS:** `ResponsiveQuery::viewport()` emits `@media` rules; `ResponsiveQuery::parent()` and `ResponsiveQuery::component(ref)` emit named `@container` rules.
- **Safe SVG by default:** Plain SVG strings emit `<img src="data:image/svg+xml;base64,...">` so untrusted SVG scripts cannot execute. Trusted inline SVG must opt in explicitly via `TrustedHtml`.
- **Keyed live DOM patching (`reconcile.js`):** Updates match live DOM children by `id`, `data-sui-key` (`.key("...")`), or `role` + `name`, preserving focus, text selection, and scroll state across list reorders.
- **Headless Chromium capture:** `HtmlRenderer::render_to_png` renders and screenshots the HTML output at exact viewport dimensions via `chromiumoxide` (see [`examples/html_native`](../examples/html_native)).

### SSR, Hydration & Authorization-Enforced Routing (`--features ssr`)

`schnellui::render_html` provides a type-safe server-rendering and routing stack:

- **`SsrChain` & `HydrationKey<T>`:** Chain server-only stages (database handles, secrets, request context) with `.then(...)`. Server state is never serialized to the client unless you explicitly pass a typed payload to `.hydrate(KEY, client_state)`.
- **`HtmlRouter`, `SsrRoute`, `CsrRoute` & mandatory `SsrAuthorize`:** Every server route requires an explicit authorization check (`SsrAuthorize`) before rendering. On the browser (`wasm32`), `HtmlRenderer::take_hydration` and `HtmlRouter::mount` hydrate into live CSR routes without page reloads.

See these complete examples:
- [`examples/html_native`](../examples/html_native) — Semantic HTML gallery + Chromium screenshot capture.
- [`examples/html_native_router`](../examples/html_native_router) & [`examples/html_native_router_ssr`](../examples/html_native_router_ssr) — Split native SSR server + WASM CSR client with explicit hydration.
- [`examples/secure_todo_ssr`](../examples/secure_todo_ssr) — No-JS Axum SSR app with Argon2 login, sessions, CSRF protection, and per-user route authorization.

---

## 4. Native Android Views (`schnellui::render_android`)

Enable with Cargo feature `render-android`.

`render_android` renders `Template` trees into **real `android.widget` views** (`LinearLayout`, `FrameLayout`, `ScrollView`, `TextView`, `Button`, `CheckBox`, `Switch`, `RadioButton`, `SeekBar`, `EditText`, `Spinner`, `ProgressBar`, `ImageView`, `android.app.Dialog`, etc.). Android owns layout, text input, IME, and TalkBack accessibility; Taffy and WGPU are not involved.

### Architecture

1. Your Rust library exports the five JNI symbols in one line:
   ```rust
   schnellui::android_app!({
       let state = AppState::new();
       move || build_template(&state)
   });
   ```
2. On every render or dispatched view event, `AndroidRenderer` diffs the new `AndroidNode` tree against the previous tree (matching by `.key(...)` first, then by position and kind) and encodes only the delta into a compact little-endian binary **op buffer** (`CREATE`, `SET_STR`, `SET_F32`, `SET_I32`, `CLEAR`, `INSERT`, `REMOVE`, `SET_ROOT`).
3. The zero-dependency Java host library (`io.github.codefionn.schnellui`, no AndroidX or Kotlin required) applies the op buffer in one JNI call per update and forwards user events (`CLICK`, `TOGGLE`, `CHANGE`, `INPUT`, `SELECT`, `DISMISS`) back to Rust.
4. Because the tree builder, differ, and op codec are pure Rust, they compile and test on desktop hosts (`cargo test -p schnellui --features render-android --lib render_android`).

For the full binary protocol, supported view kinds/properties, and Gradle build steps, see:
- [Android Renderer Specification](android-renderer.md)
- [Android Host README](../android/README.md)
- [`examples/android_native`](../examples/android_native)

---

## 5. Embedded Browser (`schnellui-servo`)

Applications that need to embed full web content inside native `schnellui` chrome can use `crates/schnellui-servo`:

- `ServoEngine` renders pages offscreen to RGBA frames.
- Frames stream directly into the shared `ImageAtlas` via `Image::dynamic_rgba_versioned` without remounting the surrounding UI.
- `App::register_focused_input_handler` forwards full-fidelity pointer, wheel, keyboard, focus, clipboard, and IME events to the focused browser surface.
- Optional JSON session persistence saves tabs, history, zoom, scroll position, and cookies.

See [`examples/servo/README.md`](../examples/servo/README.md) for build and usage commands.
