# Getting Started

This guide explains how to set up a `schnellui` project, how the retained lifecycle works, how to run apps in desktop or headless mode, and which Cargo features and environment variables control behavior.

## Requirements

- **Rust**: `rust-version = "1.97"` (2021 edition).
- **GPU (optional)**: Any Vulkan, Metal, or DX12 adapter supported by `wgpu`. If no usable GPU is found, `schnellui` automatically falls back to its built-in SIMD CPU renderer (`render_soft`).

## Adding `schnellui`

Add `schnellui` to your `Cargo.toml`:

```toml
[dependencies]
schnellui = { path = "path/to/schnellui/crates/schnellui" }
```

You do not need to depend on `schnellui-macro` directly — `schnellui` re-exports `view!` and all core modules from the umbrella crate.

For smaller debug and release binaries, add the workspace profiles described in [Binary Size](binary-size.md).

### Cargo Features

All optional backends and instrumentation gates live behind Cargo features on `schnellui`:

| Feature | Default | Enables |
| --- | --- | --- |
| `render-html` | Off | Semantic HTML/CSS renderer (`schnellui::render_html`), WASM DOM patching, and headless Chromium capture (`chromiumoxide`). |
| `ssr` | Off | Server-side rendering (`SsrChain`, `HydrationKey`, `HtmlRouter`, `SsrRoute`, `SsrAuthorize`). Implies `render-html`. |
| `render-android` | Off | Native Android view op-buffer renderer (`schnellui::render_android`) and `android_app!` JNI export macro. |
| `debug-instrumentation` | Debug builds only | Compiles and starts the local debug HTTP/Unix-socket server in `--release` builds. Debug builds enable it automatically unless `SCHNELLUI_DEBUG_SERVER=off`. |
| `count-allocations` | Off | Links the closure-scoped `allocation-counter` global allocator for allocation budget tests. |
| `fuzzing` | Off | Exposes headless fuzzing and invariant probes (`schnellui::testing::fuzz`). |

Optional companion crates in the workspace:

- `schnellui-icons-md` — Material Design SVG icons (`MdIcon`).
- `schnellui-servo` — Embedded Servo browser engine (`ServoEngine`, `Browser`).
- `schnellui-debug` — CLI client for inspecting and controlling live apps.

## Core Mental Model: Build Once, Update in Place

Every `schnellui` native application follows a three-step lifecycle:

1. **Mount (`App::mount*`)** — Your UI function runs **once**. It builds a retained tree inside `Scene`, attaches layout styles to Taffy, shapes initial text, and registers reactive subscriptions and event handlers.
2. **Frame (`App::frame`)** — Runs one synchronous settle pass over **only what is dirty**:
   ```text
   pull signals → reactive attrs & dynamic slots → layout (if layout-dirty) → paint (if paint-dirty) → a11y (if a11y-dirty)
   ```
   A clean frame does almost nothing and allocates zero bytes.
3. **Present (`App::run_windowed*` or `App::render_to_png`)** — Either opens a live event-driven native window (or headless-windowed session), or reads back one deterministic frame to PNG/RGBA8.

Because your view function is a **setup function** rather than a per-frame render loop, changing a signal does not re-execute your component function. Only the affected reactive slots update their retained nodes in place.

## Your First App

### 1. A Static View (One-Shot PNG)

```rust
use schnellui::widgets::View;
use schnellui::{view, App};

fn hello_view() -> impl View {
    view! {
        pad(all = 20.0) {
            column(gap = 6.0) {
                text(size = 32.0) { "Hello, schnellui" }
                text(size = 16.0) { "Built once, rendered cleanly." }
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::mount_with_size_scaled(hello_view(), 400, 160, 2.0);
    app.frame();
    app.render_to_png("hello.png")?;
    Ok(())
}
```

### 2. Adding State, Events, and a Desktop Window

Put reactive state in signals (`create_signal`), read them inside `(expr)` dynamic text slots or closure attributes, and mutate them in `on:<event>` handlers:

```rust
use schnellui::a11y::Role;
use schnellui::signal::create_signal;
use schnellui::widgets::View;
use schnellui::{view, App};

fn counter_view() -> impl View {
    let count = create_signal(0_i64);

    view! {
        pad(all = 20.0) {
            column(gap = 10.0) {
                text(size = 22.0) { "Counter" }
                text(role = Role::Status) { (count.get()) }
                row(gap = 8.0) {
                    button(on:click = move || count.update(|n| *n -= 1)) { "Decrement" }
                    button(on:click = move || count.update(|n| *n += 1)) { "Increment" }
                }
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::mount_with_size(counter_view(), 360, 180);
    app.set_escape_exits(true);
    app.run_windowed("Counter")?;
    Ok(())
}
```

> **Note:** In `view!`, a child wrapped in parentheses — `(expr)` — becomes a reactive text slot (`Text::dynamic_into`). An attribute set to a closure — `color = move || ...` or `disabled = move || ...` — becomes a reactive attribute binding. A plain non-closure expression like `size = my_size` is evaluated once at mount.

## Three Execution Modes

`schnellui` apps can run in three modes using the exact same UI code:

1. **One-shot headless (default in examples)** — Mounts a state, calls `app.frame()`, writes a PNG and optional AccessKit JSON tree, runs semantic assertions, and exits immediately with no event loop. Ideal for deterministic screenshots, visual regression tests, and CI.
2. **Native desktop window (`App::run_windowed*` / `--windowed`)** — Starts a `winit` event loop with reactive redraws (no busy loop), native accessibility (`accesskit_winit`), clipboard, IME, and GPU rendering (opening immediately on the CPU renderer while `wgpu` initializes on a worker thread).
3. **Live offscreen (`App::run_headless_windowed` / `--headless-windowed` / `SCHNELLUI_HEADLESS_WINDOWED=1`)** — Runs the full live application loop, timers, input handlers, and remount hooks without creating a desktop window, display connection, or touching your system clipboard. Control and screenshot it with `schnellui-debug` (see [Live Debug Instrumentation](debugging/DEBUG_INSTRUMENTATION.md)).

## Running Workspace Examples & Tests

```sh
# List scenarios in any multi-scenario example
cargo run -p playground -- --list

# Render one scenario to PNG and verify its accessibility assertions
cargo run -p playground -- --scenario gallery --scale 2 --out gallery.png --assert

# Open any example interactively
cargo run -p todo -- --scenario day_plan --windowed

# Run core tests (always pass --test-threads=1 because the signal arena is process-global)
cargo test -p schnellui -- --test-threads=1

# Run HTML/SSR or Android renderer tests
cargo test -p schnellui --features render-html,ssr --lib render_html -- --test-threads=1
cargo test -p schnellui --features render-android --lib render_android -- --test-threads=1
```

## Environment Variables

| Variable | Values / Purpose |
| --- | --- |
| `SCHNELLUI_BACKEND` | `cpu` selects the SIMD CPU renderer (`Backend::Cpu`); `software` selects a `wgpu` software rasterizer (lavapipe / SwiftShader / WARP). |
| `SCHNELLUI_SIMD` | `portable` disables AVX intrinsics in `render_soft` and uses portable vectorized arrays (useful for cross-checking output). |
| `SCHNELLUI_AUTOCLOSE_MS` | Auto-closes a windowed or headless-windowed app after `N` milliseconds (useful for smoke tests). |
| `SCHNELLUI_HEADLESS_WINDOWED` | Set to `1` to route any `App::run_windowed*` call through the offscreen live host. Requires debug instrumentation. |
| `SCHNELLUI_DEBUG_SERVER` | Set to `off` to disable automatic debug instrumentation startup in debug builds. |
| `SCHNELLUI_DEBUG_SOCKET` | Explicit Unix-domain socket path for the debug server. |
| `SCHNELLUI_DEBUG_INFO` | Explicit path for the JSON discovery record (endpoint + bearer token). |
| `SCHNELLUI_DEBUG_TRANSPORT` | Set to `tcp` on Unix to bind an ephemeral `127.0.0.1:0` port instead of a Unix socket. |
| `SCHNELLUI_DEBUG_URL` / `SCHNELLUI_DEBUG_TOKEN` | Explicit endpoint URL and bearer token for `schnellui-debug`. |
| `SCHNELLUI_INTERACTION_TRACE` | File path (or `-` / `stderr`) to write a live JSONL trace of pointer, focus, keyboard, a11y, and remount events. |
| `SCHNELLUI_INTERACTION_TRACE_TEXT` | Set to `1` to include verbatim typed/IME/`set_value` strings in interaction traces instead of lengths only. |
| `SCHNELLUI_BLESS` | Set to `1` to bless/overwrite PNG and Android golden snapshots in tests. |
| `SCHNELLUI_UPDATE_SNAPSHOTS` | Set to `1` to update HTML golden snapshots in `render_html` tests. |
| `SCHNELLUI_SIM_SEED` / `SCHNELLUI_SIM_CASES` | Replay a specific seed or override case count in deterministic `Simulator` tests. |
| `SCHNELLUI_REQUIRE_GPU` | Set to `1` to fail (rather than skip) GPU-specific tests if no `wgpu` adapter is available. |

## Next Steps

- Learn how signals, memos, stores, and structural updates work in [Reactivity & State](reactivity.md).
- Explore the `view!` syntax, layout system, widgets, and theming in [Widgets, Layout & Theming](widgets-and-layout.md).
- See how to target native GPU, CPU, HTML/SSR, and Android in [Renderers & Targets](renderers.md).
