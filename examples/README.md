# schnellui examples

Every example is a **one-shot screenshotter** (SOUL §7): it builds the UI in a named
state, renders exactly one synchronous frame, writes a PNG (and, on request, the
AccessKit tree as JSON), and exits. No event loop, no wall clock — deterministic,
diffable, machine-legible.

Read them in order; each teaches one concept on top of the last.

| # | Example | Teaches | Key APIs |
|---|---------|---------|----------|
| 1 | [`hello`](hello/) | The smallest complete program: mount → one frame → PNG | `view!`, `App::mount_with_size_scaled`, `App::frame`, `App::render_to_png` |
| 2 | [`counter`](counter/) | Signals, dynamic text slots, driving state through AccessKit clicks (§7.5) | `create_signal`, `Text::dynamic`, `Button::on_click`, `App::dispatch_action` |
| 3 | [`temperature`](temperature/) | Derived state: one source signal, a memo computed from it | `create_memo`, push-then-pull (§3.1), drive + a11y assertions |
| 4 | [`settings`](settings/) | Stateful widgets; the a11y tree as the test oracle | `Checkbox::on_toggle`, checked state, `Role`+name queries |
| 5 | [`calculator`](calculator/) | A complete signal-driven app: state-machine actions, chained arithmetic, error states, and a custom design system | `Signal<String>`, `Button::on_click`, `Theme`, driven key sequences |
| 6 | [`todo`](todo/) | A complete structural app: text entry, add/remove/complete/clear actions, and same-window remounts for a dynamic task list | `TextInput::on_input`, `Checkbox::on_toggle`, `App::run_windowed_with`, `SetValue` |
| 7 | [`layout_gallery`](layout_gallery/) | The layout layer: where/how-big, never draws (§8.1); `wrap` — one paragraph as NoWrap / Word / ellipsis / centered, so wrapped text's height is measured through layout; plus responsive flex — `flex(grow = n)` weighted shares and `row(wrap)` line flow | `Row`/`Column` + `gap`/`width`/`justify`, `Pad`, `Stack`, `Spacer`, `Flex`, `row(wrap)`, `Text::wrap`/`align`/`ellipsis` |
| 8 | [`flexbox`](flexbox/) | Responsive layout: geometry that re-derives from the padded viewport — the toolbar grown-gap, weighted 1:2:3 shares, four `justify` distributions, a wrapping card grid, a pinned footer; render at two `--width`s or resize the `--windowed` window to watch it adapt live | `column(fill, align = stretch)` work-area root, `flex(grow = n)`, `row(wrap)`, `justify = start/center/end/space_between` |
| 9 | [`playground`](playground/) | The whole component library on one stage: a `gallery` scenario with every widget, focused driven scenarios for each interactive family, typography and image stages, plus an embedded decorated/undecorated dialog comparison with wrapped body copy. Every state is asserted from the a11y tree. The page chrome carries the **Theme dropdown**: in `--windowed` mode picking a palette remounts the same scenario with a different design system on the fly (`--theme` selects it headlessly). | `button`/`link`/`badge`, inputs and selections, `Theme`/`set_theme`, tables, raster/SVG images, `Dialog`, `WrapMode` |
| 10 | [`dialogs`](dialogs/) | Seven editorial dialog screenshots: decorated and undecorated modals, a modeless inspector, a parent-scoped modal, a persistent alert, a stacked accessibility case, and a desktop workspace with three independently closable, title-bar-movable, resizable non-fixed windows. The focused/pressed desktop window is raised within the shared modeless plane, and that same order governs painting and input. Assertions cover roles, modal state, focus capture, inert peers, chrome, wrapping, geometry, movement, resizing, and foreground hit-testing. | `Dialog`, `DialogPosition`, `modal`/`modeless`, `fixed`/`non_fixed`, `movable`, `resizable`, `decorated`/`undecorated`, `persistent`, `alert`, `WrapMode::Word` |
| 11 | [`dockable_workspace`](dockable_workspace/) | A configurable personal dashboard with direct spatial docking: reorder tabs within a pane, drop a tab on a pane edge to split left/right/top/bottom, or drop it on another pane's tab to merge. | `TabBar::on_reorder`, `DockArea::on_dock`, `DockPosition`, `Tab::on_drag_start`, `Tab::on_drop`, `App::begin_drag`/`update_drag`/`end_drag`, `App::run_windowed_with` |
| 12 | [`md_icons`](md_icons/) | A Material Design icon gallery exercising the library-neutral icon-source seam, three `md-icons` families and their filled variants, multiple physical sizes, accessible labels, draw-time tinting, and shared CPU/GPU cache entries. | `IconSource`, `Icon`, `MdIcon`, `SvgCacheKey`, `cache_stats` |
| 13 | [`grouped_tabs`](grouped_tabs/) | An interactive project navigator built from the same grouped tab data in flat, expanded-tree, collapsed-tree, and driven nested-selection states. Branch rows fold/unfold through a controlled remount, while each row can expose one or several programmer-defined archive/delete actions with independent callbacks and matching hover/screen-reader labels. | `GroupedTabList`, `TabNode::expanded`, `TabNode::on_toggle`, `TabNode::action`, `TabNode::actions`, `Button::icon_only`, `Button::tooltip`, `MdIcon`, `App::run_windowed_with` |
| 14 | [`html_native`](html_native/) | A renderer-generic base-component tree rendered as semantic HTML/CSS and captured at an exact viewport through Chromiumoxide. No canvas or WGPU scene translation is involved. | `schnellui::template`, `HtmlRenderer::render`, `HtmlRenderer::render_to_png`, `App::mount_template` |
| 15 | [`html_native_router`](html_native_router/) | WASM CSR router and shared hydration contract for the native-HTML router examples. | `HtmlRouter`, `CsrRoute`, `HtmlRenderer::take_hydration` |
| 16 | [`html_native_router_ssr`](html_native_router_ssr/) | Separate native SSR router crate with mandatory authorization, chained server-only state, explicit CSR hydration, and a nested server-derived view. | `SsrRoute`, `SsrAuthorize`, `SsrChain::then`, `SsrChain::hydrate` |
| 17 | [`secure_todo_ssr`](secure_todo_ssr/) | A no-JavaScript SSR todo application with password login, server-side sessions, CSRF protection, per-user authorization, and HTTP-level security tests. | `HtmlRouter`, `SsrAuthorize`, Axum, Argon2 |
| 18 | [`servo`](servo/) | A real Servo 0.4 webview rendered into SchnellUI chrome. Headless mode captures a deterministic local page; windowed mode forwards pointer, keyboard, wheel, focus, and IME input and remounts new browser frames. | `ServoEngine`, `Browser`, `BrowserFrame`, `App::register_focused_input_handler`, `App::run_windowed_with_viewport` |

## The common CLI contract (SOUL §7.1)

All multi-scenario examples share it. `hello` and `md_icons` are deliberately
single-stage and expose only output/viewport/scale/a11y/windowed flags:

```
<example> --list                              # print scenario names, one per line
<example> --scenario <name> --out <path.png>  # render one state
<example> --all --out-dir DIR [--manifest m.json]
<example> --dump-a11y <path.json>             # the semantic tree: what the UI *is*
<example> --assert                            # run the scenario's a11y assertions
<example> --width W --height H --scale S
<example> --scenario <name> --windowed        # opt-in: open a real window instead
```

`--windowed` (opt-in, never the default) opens the scenario in a real window —
winit event loop, vsync, reactive redraws only (a redraw is requested after input,
never a busy loop), and real mouse clicks routed through the **same** handlers as
AccessKit `ActionRequest`s (§6.3). Close with Esc or the window button;
`SCHNELLUI_AUTOCLOSE_MS=<n>` auto-closes for CI/agent smoke tests.

The native HTML example has the same output/viewport/scale shape, but its capture
is async because Chromiumoxide owns a browser process:

```bash
cargo run -p html_native -- --out html-native.png --width 520 --height 260
```

The Servo example renders a webview offscreen and can either capture the composed
SchnellUI window or keep both renderers live in a native window:

```bash
cargo run -p servo_demo --release -- --out servo.png
cargo run -p servo_demo --release -- --url https://servo.org --windowed
```

For the dockable workspace:

```bash
cargo run -p dockable_workspace -- --scenario starter --windowed
cargo run -p dockable_workspace -- --scenario right_preview --out right_preview.png --assert
```

For grouped/tree tabs:

```bash
cargo run -p grouped_tabs -- --list
cargo run -p grouped_tabs -- --scenario tree --windowed
cargo run -p grouped_tabs -- --scenario nested_selected --out grouped-tabs.png --assert
cargo run -p grouped_tabs -- --scenario tree --hover-action "Delete Notes" --out grouped-tabs-hover.png
```

The PNG shows how the UI **looks**; `--dump-a11y` shows what it **is** (role, name,
value, state, focus). `--assert` makes the a11y tree the primary correctness oracle —
scenarios that are *driven* (counter's `counter_five`, temperature's `warmer`,
settings' `all_enabled`) reach their state by dispatching real AccessKit
`ActionRequest`s located by Role+name, proving the state is reachable through the
same handlers a mouse or screen reader would fire (§6.3).

## Idioms worth stealing

- **`view!` first, builders when you need them.** The macro covers the common tree
  (`column { text(size = 24.0) { "…" } button(on:click = …) { "…" } }`); the typed
  builder chain (`Column::new().child(…)`) is the same thing spelled out, and the
  right tool when you need what the grammar doesn't cover yet (fixed container
  sizes — see `layout_gallery`'s `stack`/`spacer`).
- **A component function runs once** (§3.3). Reactivity lives only in the closures
  you hand to `Text::dynamic` / `create_memo` — there is no re-render of your code.
- **Derive, don't duplicate.** `temperature` keeps one `celsius` signal; fahrenheit
  is a memo. A second signal could drift; a memo can't.
- **Assert semantics, not pixels.** All `--assert` checks query the a11y tree; the
  PNG is the secondary, perceptually-diffed artifact (§7.4).
- **Frame the lesson.** Utility examples use a 20px window gutter, keeping the
  first label and last control away from every physical edge. Branded examples
  may use a larger composition-specific margin; modal backdrops remain full-bleed.
- **Dialogs are explicit about behavior.** `Dialog::new("Title")` is a fixed,
  centered modal by default; compose `.modeless()`, `.non_fixed()`,
  `.undecorated()`, `.position(DialogPosition::BottomRight)`, or `.persistent()`
  for inspectors, parent-scoped surfaces, chrome-free panels, alternate placements,
  and non-dismissible workflows. `on_dismiss` receives both backdrop and Escape
  close requests.
- **Wrapped text is width-dependent height** (§8.1). A wrapping `Text` can't know how
  tall it is until layout hands it a width, so — unlike a single-line leaf sized at
  build — it's *measured through the layout pass* and re-wraps on resize. Give it a
  definite-width ancestor to wrap against (`layout_gallery`'s `wrap` uses a fixed-width
  column); `NoWrap` keeps the unchanged single-line path and simply overflows.
- **Responsive visibility is node-transparent.** Import `View as _`, then write
  `Button::new("Export").show_when(ResponsiveQuery::viewport().min_width(em(48.0)))`
  for a window breakpoint, or use `ResponsiveQuery::parent()` for an immediate-parent
  container query. Bounds are inclusive, may combine `min/max_width/height`, and
  accept logical pixels directly (`640.0` or `px(640.0)`) or CSS-style `em`.
  For a specific ancestor, create `let card = ComponentRef::new()`, attach it with
  `.with_ref(card)`, and query it with `ResponsiveQuery::component(card)`; the same
  handle resolves to the current mount's node through `app.resolve_ref(card)`.
  Give a queried parent a definite/fill-derived size; the HTML renderer emits real
  `@media`/named `@container` rules, while retained rendering resolves the same rule
  during layout. In `view!`, use the same extension as an attribute:
  `button(show_when = ResponsiveQuery::viewport().max_width(40.0)) { "Menu" }`.
  Reference a container with
  `component_ref(value = card) { column { /* descendants */ } }`.
