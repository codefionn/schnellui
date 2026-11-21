# Accessibility, Testing & Agent Workflows

In `schnellui`, accessibility, automated testing, and AI-agent legibility are built on the same foundation: **every widget is semantic before it is visual**.

---

## 1. First-Class Accessibility (`schnellui::a11y`)

### Semantic-First Widgets

Every content widget records its AccessKit metadata directly in the retained `Scene`'s a11y column:

- **`Role`** (`Button`, `TextInput`, `CheckBox`, `Switch`, `RadioButton`, `Slider`, `List`, `Table`, `Dialog`, `Status`, `Link`, `Chart`, …)
- **Accessible name & value** (label text, field text, formatted chart summary, terminal grid text)
- **State flags** (`FOCUSED`, `DISABLED`, `CHECKED`, `SELECTED`, `EXPANDED`, `MODAL`, `INVALID`, …)
- **Supported actions** (`Click`, `Focus`, `Blur`, `SetValue`, `Increment`, `Decrement`, `ScrollUp`, `ScrollDown`, `ShowContextMenu`)
- **Bounds** (updated automatically when layout or composited transforms change)

### Incremental Updates

Each retained `WidgetId` maps 1:1 to its AccessKit `NodeId` (`to_access_id`). On every frame, the native host (`accesskit_winit` — UI Automation on Windows, NSAccessibility on macOS, AT-SPI on Linux) picks the minimal update:

- **`Skip`** — Nothing is `a11y_dirty` and focus did not move.
- **`Incremental`** — Emits a `TreeUpdate` containing **only** nodes on `scene.a11y_dirty()`.
- **`Full`** — First publish, remount, resize, title change, structural revision bump, or active modal change.

### Bidirectional Actions

Assistive technologies send inbound `accesskit::ActionRequest`s. `App::dispatch_action` routes them through the **exact same handlers** as mouse and keyboard input:

```text
Mouse / Keyboard    ─┐
AccessKit Action    ─┼─► Widget dispatch ─► Callback / State mutation ─► Dirty frame ─► Pixels + TreeUpdate
DriveAction (tests) ─┘
```

Because both paths converge, screen-reader interaction, keyboard navigation (tab order, roving list focus, modal focus traps, focus return on close, IME composition), and automated tests all exercise the real application logic.

---

## 2. Semantic Testing (`schnellui::testing`)

### Driving and Asserting by Role + Name

Rather than clicking brittle pixel coordinates, tests locate controls by semantic `Role` and accessible name and assert against the AccessKit tree dump:

```rust
use schnellui::a11y::{self, Role};
use schnellui::template::DriveAction;
use schnellui::testing::{assert_enabled, assert_value_contains, find_by_role_name};
use schnellui::App;

let mut app = App::mount_with_size(counter_view(), 400, 200);
app.frame();

// Drive through the real handler path (same as mouse click or screen reader)
for _ in 0..5 {
    assert!(app.drive_action(&DriveAction::click(Role::Button, "Increment")));
}
app.frame();

// Assert against the semantic tree
let tree = a11y::dump_tree(app.scene());
assert_value_contains(&tree, "status", None, "5").unwrap();
assert_enabled(&tree, "button", Some("Increment")).unwrap();
assert!(find_by_role_name(&tree, "button", Some("Increment")).is_some());
```

For lower-level keyboard and pointer testing without a window, `App` also provides `dispatch_action`, `dispatch_key(UiKey::...)`, `dispatch_pointer_*`, `dispatch_wheel_at`, `dispatch_scroll`, and `dump_a11y(path)`.

### Deterministic Headless Screenshots & Golden Diffing

Every `App` can render a single synchronous frame to an offscreen RGBA8 buffer or PNG with a pinned viewport, scale factor, embedded fonts, and logical clock `now = 0`:

```rust
use schnellui::testing::{bless_or_compare, SnapshotConfig};

app.frame();
let (rgba, width, height) = app.render_rgba8()?;

let cfg = SnapshotConfig::from_env("snapshots"); // reads SCHNELLUI_BLESS=1
let diff = bless_or_compare("counter_five", &rgba, width, height, &cfg)?;
assert!(diff.matches, "visual diff score {:.4} exceeded tolerance", diff.score);
```

- On mismatch, `bless_or_compare` writes `snapshots/<name>.diff.png` highlighting changed pixels in red so a developer or vision-capable agent can inspect the delta.
- Re-bless goldens with `SCHNELLUI_BLESS=1`.
- For cross-machine determinism, select `Backend::Software` (`SCHNELLUI_BACKEND=software`) or `Backend::Cpu` (`SCHNELLUI_BACKEND=cpu`).

### Deterministic Simulation & Fuzzing

`schnellui::testing::Simulator` and `crates/schnellui-fuzz` test layout, resize equivalence, tab-stop reachability, sideways clipping at mobile widths (e.g. 360 px), and randomized interaction sequences:

- Replaying a failing simulation seed: `SCHNELLUI_SIM_SEED=0x... cargo test ... -- --test-threads=1`
- Overriding case count: `SCHNELLUI_SIM_CASES=500`
- Running the fuzz suite: `cargo test -p schnellui-fuzz -- --test-threads=1`

---

## 3. Allocation Budgets & Scaling Gates

Zero allocations on steady-state re-render is tested continuously in two places:

### 1. Unit Allocation Budgets (`alloc_budget`)

```sh
cargo test -p schnellui --features count-allocations \
    --test alloc_budget --test view_reactive_attrs --test text_area_edit_allocations \
    -- --test-threads=1
```

Measures the **second** steady-state invocation (so warm-capacity growth cannot hide regressions) and asserts `count_total == 0 && bytes_total == 0 && count_current == 0` on zero-budget rows, and fixed reviewed ceilings on budgeted rows (such as text re-wrap and AccessKit `TreeUpdate`).

### 2. Allocation & Document-Size Scaling Benchmark (`schnellui-bench`)

```sh
# Full budget + scaling tables (exits non-zero on any regression)
cargo run --release -p schnellui-bench

# Allocation counts and bytes only (deterministic gate used in CI)
cargo run --release -p schnellui-bench -- --allocations-only --iters 100

# Inspect available paths or emit JSON
cargo run --release -p schnellui-bench -- --list
cargo run --release -p schnellui-bench -- --filter text_edit --json
```

`schnellui-bench` verifies both:
- **Per-path budgets** (19 literal-zero rows, 3 bounded rows, 11 report-only mount/grow rows).
- **Document-size proportionality** (19 scale curves tested at `n = 10`, `100`, `1000`): allocation counts must stay identical across `n` and median time ratio `n=1000 / n=10` must stay flat (`< 3×`), proving work is proportional to what changed, not tree size.

---

## 4. The AI-Agent Workflow

`schnellui` is designed so an AI agent can inspect, drive, verify, and iterate on UI without needing a human to describe the screen or take over your desktop.

### One-Shot Screenshotter CLI Contract

Multi-scenario examples expose a uniform CLI:

```sh
# 1. Discover available states
cargo run -p playground -- --list

# 2. Render every scenario to PNG + a machine-readable manifest
cargo run -p playground -- --all --out-dir /tmp/shots --manifest /tmp/shots/manifest.json

# 3. Inspect both pixels AND semantics for one scenario, and run its assertions
cargo run -p counter -- --scenario counter_five \
    --out /tmp/counter.png --dump-a11y /tmp/counter.json --assert
```

### Live Offscreen Automation (`--headless-windowed` + `schnellui-debug`)

For interactive multi-step flows (typing, IME preedit/commit, hovering, dragging sliders/dialogs/tabs, resizing, triggering remounts), launch any app with `--headless-windowed` (or `SCHNELLUI_HEADLESS_WINDOWED=1`) and drive it via `schnellui-debug`:

- Opens **no native window**, requires **no Xvfb/compositor**, and never touches the user's focus or system clipboard.
- Executes every command synchronously on the UI thread and settles a frame before replying — no arbitrary sleeps needed.
- Supports `status`, `tree`, `snapshot`, `capture`, `wait`, `click`, `action`, `move`, `click-at`, `press`, `release`, `drag`, `leave`, `wheel`, `resize`, `ime`, `key`, `type`, `global-shortcut`, `global-menu-item`, `screenshot`, `quit`, embedded `--jq` filters, and multi-line `script` files.

See:
- [Live Debug Instrumentation](debugging/DEBUG_INSTRUMENTATION.md) for the complete CLI and HTTP/Unix-socket protocol reference.
- [Interaction & Remount Tracing](debugging/INTERACTION_DEBUGGING.md) for recording JSONL diagnostics of pointer hit paths, focus changes, interrupted drags, and reasoned remounts.
- [Live offscreen launch & cleanup recipe](../examples/README.md#live-apps-without-a-desktop-window) for a copy-pasteable shell script.
