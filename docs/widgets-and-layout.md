# Widgets, Layout & Theming

`schnellui` separates **layout** from **content widgets**. Both compose seamlessly inside the same `view!` tree or typed builder chain, but they have strictly separated responsibilities and dirty channels.

---

## 1. Two Layers: Layout vs Content Widgets

| Concern | Layout (`schnellui::layout`) | Content Widgets (`schnellui::widgets`) |
| --- | --- | --- |
| Question answered | *Where* and *how big* | *What* is on screen |
| Examples | `column`, `row`, `stack`, `flex`, `pad`, `spacer`, `scroll` | `text`, `button`, `checkbox`, `text_input`, `table`, `dialog`, `charts` |
| Draws pixels? | No — computes rects and transforms only | Yes — emits scene primitives |
| Accessibility | Structural / transparent (or `Group`) | Always carries an AccessKit `Role`, name, value/state, and actions |
| Intrinsic size | Derived from children and constraints | Measures itself (e.g., shaped text metrics) |
| Engine | Taffy (Flexbox) | Retained scene widgets + Parley text |
| Dirty channel | `layout_dirty` | `paint_dirty` + `a11y_dirty` |

Because the layers are orthogonal:
- Changing a color or same-width text marks only `paint_dirty` (and `a11y_dirty`) — **no relayout**.
- Changing scroll offset or composited transform updates property/scroll state — **no relayout**.
- Changing text length, wrapping width, or child structure marks `layout_dirty` and lets Taffy re-place only affected geometry.

---

## 2. Layout Primitives & Responsive Queries

### Containers

- **`column` / `Column`** — Vertical flex stack.
- **`row` / `Row`** — Horizontal flex stack. Supports `wrap` for multi-line card grids and toolbars.
- **`stack` / `Stack`** — Overlapping layers placed on top of one another.
- **`scroll` / `Scroll`** — Clipping scrollable viewport. Supports `.scrollbar(true)`, `.edge_auto_scroll(true)`, `.follow_end(true)`, `.restoration_key(...)`, and debounced scroll callbacks.
- **`pad` / `Pad`** — Adds padding around a single child (`pad(all = 20.0)` or `Pad::xy(h, v)` / `Pad::trbl(...)`).
- **`spacer` / `Spacer`** — Flexible empty space that pushes siblings apart.
- **`flex` / `Flex`** — Node-transparent wrapper that applies flex child properties (`grow`, `shrink`, `basis`, `width`, `height`, `min_width`, `min_height`, `max_width`, `max_height`) to its single child, or acts as a weighted spacer when childless.

Common container attributes:
- `gap = <f32>` — Space between adjacent children.
- `justify = start | center | end | space_between | space_around | space_evenly` — Main-axis distribution (`Justify`).
- `align = start | center | end | stretch` — Cross-axis alignment (`Align`).
- `fill` — Fill the available space of the parent.
- `wrap` — Allow children to wrap onto multiple lines.
- `width`, `height`, `min_width`, `min_height` — Explicit or minimum logical pixel dimensions.

### Wrapped Text & Width-Dependent Height

Single-line `Text` (`WrapMode::NoWrap`, the default) measures its intrinsic size at build time. Wrapped text (`WrapMode::Word` or `WrapMode::Anywhere`) depends on the width layout gives it, so it is measured width-aware during the layout pass and automatically re-wraps on resize. Give wrapped text a definite-width or stretched ancestor:

```rust
view! {
    column(width = 320.0, align = stretch, gap = 8.0) {
        text(wrap = word) { "This paragraph wraps cleanly inside the 320px column." }
        text(wrap = nowrap, ellipsis) { "A long single-line label that truncates with an ellipsis" }
    }
}
```

### Responsive Visibility & Component References

Any view (and any `Template`) supports node-transparent responsive queries without introducing an extra wrapper node:

```rust
use schnellui::widgets::{em, px, Button, Column, ComponentRef, ResponsiveQuery, View as _};

let card = ComponentRef::new();

// Viewport breakpoint (inclusive bounds; accepts logical px or em)
let export_btn = Button::new("Export")
    .show_when(ResponsiveQuery::viewport().min_width(em(48.0)));

// Immediate-parent container query
let compact_badge = Button::new("Compact")
    .show_when(ResponsiveQuery::parent().max_width(px(360.0)));

// Specific ancestor container query via ComponentRef
let container = Column::new()
    .width(420.0)
    .child(
        Button::new("Wide Card Action")
            .show_when(ResponsiveQuery::component(card).min_width(400.0))
    )
    .with_ref(card);
```

In `view!`:
```rust
view! {
    component_ref(value = card) {
        column(width = 420.0) {
            button(show_when = ResponsiveQuery::viewport().max_width(640.0)) { "Menu" }
        }
    }
}
```

On the retained native path, `ResponsiveQuery` resolves during layout; on the HTML renderer, it emits real CSS `@media` and named `@container` rules.

---

## 3. `view!` Macro Syntax Reference

The `view!` macro (parsed by `schnellui-view-parser` and expanded by `schnellui-macro`) compiles to the exact typed `schnellui::widgets` builder chain plus a hoisted `const TEMPLATE` descriptor for static text and hot reload.

### Grammar

```text
node    = "string literal"                    // static text (hoisted)
        | "(" rust_expr ")"                   // dynamic text slot -> Text::dynamic_into
        | element
element = tag ("(" attr, ... ")")? ("{" node* "}")?
attr    = name "=" expr                       // static literal, builder arg, or reactive closure
        | name                                // valueless flag -> .name()
        | "on:" event "=" handler_expr        // event handler -> .on_<event>(handler_expr)
```

A `view! { ... }` block must produce **one root node**.

### Supported Elements

| Category | `view!` tag (and aliases) | Builder type |
| --- | --- | --- |
| Layout | `column`, `row`, `stack`, `scroll`, `flex`, `pad`, `spacer`, `component_ref` (`referenced`) | `Column`, `Row`, `Stack`, `Scroll`, `Flex`, `Pad`, `Spacer`, `Referenced` |
| Text & Media | `text`, `image`, `icon`, `svg`, `divider`, `badge` | `Text`, `Image`, `Icon`, `Svg`, `Divider`, `Badge` |
| Actions & Nav | `button`, `link`, `tabs` (`tab_bar`, `tabbar`), `tab`, `grouped_tabs` (`grouped_tab_list`, `grouped_tablist`), `tab_group` (`tabgroup`), `tab_node` (`tabnode`, `tree_tab`) | `Button`, `Link`, `TabBar`, `Tab`, `GroupedTabList`, `TabGroup`, `TabNode` |
| Inputs | `checkbox`, `switch`, `radio`, `slider`, `text_input` (`textinput`), `password_input` (`passwordinput`) | `Checkbox`, `Switch`, `Radio`, `Slider`, `TextInput`, `PasswordInput` |
| Progress | `progress`, `spinner` (`loading_spinner`) | `ProgressBar`, `LoadingSpinner` |
| Collections | `list`, `list_item` (`listitem`), `table`, `table_row` (`tr`), `table_column` (`th`) | `List`, `ListItem`, `Table`, `TableRow`, `TableColumn` |
| Overlays | `dialog` | `Dialog` |

### Keyword Attributes

Certain enum attributes accept bare keywords in `view!` (while still allowing qualified Rust expressions):

- `column` / `row` / `stack`: `justify = start | center | end | space_between | space_around | space_evenly`, `align = start | center | end | stretch`
- `text`: `wrap = nowrap | word | anywhere`, `align = start | center | end | justify`
- `dialog`: `position = center | top_left | top_right | bottom_left | bottom_right | top | bottom | left | right`
- `table_column` / `th`: `sort = asc | ascending | desc | descending`

### Reactive Attributes in `view!`

Any closure passed to one of these attributes lowers to `.reactive_attr(...)`:
`color`, `size` / `font_size`, `width`, `height`, `min_width`, `min_height`, `max_width`, `max_height`, `grow`, `shrink`, `basis`, `gap`, `padding` / `all`, `disabled`, `checked`.

### When to Use Builders Directly

`view!` and the builder API produce the exact same types and can be mixed freely. Reach for builders directly for widgets that carry structured Rust data or callbacks not covered by the macro grammar — such as `Form`, `Dropdown`, `TextArea`, `RichText`/`RichDoc`, `VirtualList`, `DockArea`, `TerminalGrid`, `ContextMenu`, `ThemeProvider`, `BarChart`/`LineChart`/`Sparkline`, or `schnellui::icons::Icon<MdIcon>`.

### Opt-In Hot Reload

Enable template hot reload on a native app with `app.enable_hot_reload("127.0.0.1:7777")`. It accepts newline-delimited JSON `TemplateBlueprint` updates and applies edits to static text, supported literal attributes, and static container/text children without recompiling Rust — while keeping dynamic nodes and subscriptions intact. Watch a `view!` body file with:

```sh
cargo run -p schnellui-view-parser --example hot_reload -- \
  --watch view.body 'src/main.rs:20:16' 127.0.0.1:7777
```

Edits that change Rust expressions, closures, or unsupported widget types return `ReloadError::RebuildRequired`.

---

## 4. Widget Catalog

Every widget below lives in `schnellui::widgets` unless noted otherwise.

### Text, Rich Text & Media
- **`Text`** — Static or dynamic (`Text::dynamic_into`) text with font size, color, wrapping (`WrapMode`), alignment (`TextAlign`), `ellipsis`, and semantic `Role` (e.g., `Role::Status`, `Role::Heading`).
- **`RichText` & `RichDoc`** — Multi-span styled document viewer (sans/mono weights, italics, inline links via `RichSpan::link_to` + `RichText::on_link`). Links become real focusable/clickable child nodes with exact per-line hit-testing.
- **`Image`** — Raster images (PNG/RGBA) and streamed frames via `Image::dynamic_rgba_versioned(revision, frame)` (updates the image atlas sub-rect in place with zero allocations for same-size frames).
- **`Svg` & `schnellui::icons::Icon<S>`** — Vector graphics with process-wide parsed & rasterized caching and draw-time tinting over alpha masks. Pair with `schnellui-icons-md` (`MdIcon`) for Material Design icons.
- **`Badge` & `Divider`** — Status pills and horizontal/vertical hairline separators.

### Controls & Forms
- **`Button`** — Solid (accent) or ghost appearance, icon-only mode, tooltips (`Button::tooltip`), `.submit()` for forms, and `on_click`.
- **`Link`** — Inline semantic navigation link with underline, focus ring, pointer cursor, and `on_click`.
- **`Checkbox`**, **`Switch`**, **`Radio`** — Toggle and exclusive-choice controls with keyboard/pointer/a11y activation.
- **`Slider`** — Continuous or stepped range input (`min`, `max`, `step`, keyboard arrows/PageUp/PageDown/Home/End, pointer drag, `on_change`).
- **`TextInput`** & **`PasswordInput`** — Single-line editable text with selection, clipboard copy/cut/paste, undo/redo, IME preedit/commit, context menu, and `.required(true)` validation.
- **`TextArea`** — Multi-line editable text with optional custom syntax highlighting, undo/redo, IME support, and viewport-banded painting so 2,000+ line documents edit and move the caret in O(1) relative to document length.
- **`Dropdown`** — Combo-box trigger with an overlay option list, keyboard arrow/type navigation, and selection callback.
- **`Form`** — Semantic `Role::Form` column with `.on_submit(...)`. Validates `.required(true)` fields in tree order on submit button click or Enter, marking missing fields `StateFlags::INVALID` and focusing the first invalid control.
- **`ProgressBar`** & **`LoadingSpinner`** — Determinate and indeterminate progress indicators (respects platform reduced-motion settings).

### Navigation, Collections & Overlays
- **`TabBar` / `Tab`** — Exclusive tab strips with drag-to-reorder (`TabBar::on_reorder`) and cross-pane dragging (`Tab::on_drag_start`, `Tab::on_drop`).
- **`GroupedTabList` / `TabGroup` / `TabNode`** — Sectioned flat or collapsible tree navigators with per-row action buttons (`TabNode::action` / `actions`), tooltips, and icons.
- **`List` / `ListItem` / `ListRow`** — Single-selection lists with roving keyboard focus, leading marks, tooltips, and row-wide context menus.
- **`VirtualList` / `VirtualListController<K>`** — Variable-height virtualized list that mounts only visible rows plus overscan and preserves widget state by key.
- **`Table` / `TableRow` / `TableColumn`** — Multi-column data tables with header rows, sortable columns (`SortDirection`), row selection, and full table a11y semantics.
- **`Dialog`** — Floating overlay surface. Fixed centered modal by default; composable with `.modeless()`, `.non_fixed()`, `.movable()`, `.resizable()`, `.decorated()` / `.undecorated()`, `.position(DialogPosition::...)`, `.persistent()`, and `.on_dismiss(...)`. Handles backdrop click, Escape dismissal, modal focus trapping, focus return on close, and stacked z-ordering.
- **`DockArea`** — Spatial docking container (`DockPosition::Center | Left | Right | Top | Bottom`) with live drop-zone previews and zone labels.
- **`ContextMenu`** — On-demand overlay menus with Cut/Copy/Paste/SelectAll, custom actions, and interactive slider rows.

### Data Visualization & Specialized Surfaces
- **`schnellui::charts`** — `BarChart`, `LineChart`, and `Sparkline`. Colorblind-safe `SERIES` palette (WCAG 1.4.11 ≥ 3:1), optional `.axes(true)`, `.labels(...)`, `.tooltips(true)` with mark highlighting, and deterministic accessible summaries (`Role::Chart`).
- **`TerminalGrid`** — Fixed-cell terminal viewport supporting `new`, `dynamic`, `dynamic_versioned`, and zero-alloc in-place `patched` row updates with inline `CompactString` graphemes and ranged GPU uploads.

---

## 5. Design System & Theming

A `Theme` (`schnellui::widgets::Theme`) defines both **semantic color tokens** (`text`, `text_muted`, `surface`, `surface_muted`, `separator`, `outline`, `accent`, `on_accent`, `selection`, `text_selection`, `disabled`, `positive`, `attention`, `media`, `page`, plus `interactions` and per-component overrides) and **physical `Shape` tokens** (`roundness`, `density`, `frame`, `shadow`).

Swapping a theme reshapes geometry as well as colors — for example, squaring corners and adding hard shadows (`BRUTAL`) or rounding controls into pills (`CANDY`).

### Built-In Themes (`schnellui::theme`)

- `theme::LIGHT` — Cool slate light palette (`Shape::CLASSIC`, default).
- `theme::DARK` — Deep neutral dark surfaces with lifted blue accent.
- `theme::FOREST` — Warm light palette with forest green accent.
- `theme::BRUTAL` — Neo-brutalist square controls, 2 px ink frames, and 4 px hard block shadows.
- `theme::CANDY` — Pastel palette with pill-shaped controls.

Every theme automatically enforces WCAG contrast helpers (`focus_color()`, `invalid_color()`, `link_color()`, `track_color()`, `indicator_color()`, `ink_on(...)`) so focus rings, links, and error outlines remain legible even on custom or pastel palettes.

### Dynamic & System-Aware Theming

Use `App::mount_themed` so the app can follow OS light/dark mode, react to a signal, or animate between themes while preserving the glyph atlas and widget state:

```rust
use std::time::Duration;
use schnellui::{theme, App, ThemeMode};

let mut app = App::mount_themed(
    ThemeMode::system(theme::LIGHT, theme::DARK),
    move || build_ui(),
);

// Optional animated transition on OS dark-mode switch (respects reduced motion):
app.set_theme_transition_duration(Duration::from_millis(180));

// Or switch programmatically / bind to a signal:
// app.set_theme(theme::FOREST);
// app.transition_theme(theme::BRUTAL, Duration::from_millis(200));
// app.bind_theme(move || selected_theme.get());
```

To style just one subtree differently (such as a dark sidebar inside a light app), wrap that subtree in `ThemeProvider::new(theme::DARK, child)`.

---

## 6. Localization, Motion & Platform Integration

### Localization (`schnellui::localization`)

Renderer-independent BCP-47 locale parsing, deterministic negotiation, fallback catalogs, and single-pass `{name}` placeholder interpolation:

```rust
use schnellui::localization::{Catalog, Locale};

let en = Locale::parse("en").unwrap();
let de = Locale::parse("de").unwrap();

let catalog = Catalog::new(en.clone())
    .with_message(en, "greeting", "Hello, {name}!")
    .with_message(de, "greeting", "Hallo, {name}!");

let loc = catalog.localizer(&[Locale::parse("de_AT.UTF-8").unwrap()]);
assert_eq!(loc.format("greeting", &[("name", &"Ada")]), "Hallo, Ada!");
// Use `loc.format_into(&mut buf, ...)` for zero-allocation formatting into a reused buffer.
```

### Declarative Motion (`schnellui::motion`)

`Motion` declares frame-rate-independent animations (`Motion::rotate`, `Motion::fade`, `Easing`, `Repeat`) shared across backends:
- On native GPU/CPU, sample `motion.value(elapsed_ms)`.
- On HTML, emit `@keyframes` and CSS shorthand via `css_animation(&motion, "name")`.
- Reduced motion is enforced automatically by native hosts (`App::apply_reduced_motion`) and CSS media queries.

### Platform Integration (`schnellui::platform`)

- **Preferences**: `PlatformPreferences` and `watch_preferences` (color scheme, high contrast, reduced motion, locale) via XDG portals on Linux, AppKit on macOS, and Win32 on Windows.
- **Clipboard**: `SystemClipboard` (text and RGBA images via `arboard`, opened lazily on first use).
- **File Dialogs**: `pick_folder` (native folder chooser off the UI thread).
- **Global Shortcuts**: `GlobalShortcuts` / `App::register_global_shortcut` (system-wide hotkeys that fire while unfocused; call `platform::set_app_id` first on Linux).
- **Global Menu Bar**: `MenuBar` / `Menu` / `MenuAction` / `StandardItem` / `App::set_global_menu` (native system menu bar on macOS; never drawn inside the window).
- **Window Chrome**: `platform::windows_always_undecorated`, `App::set_window_decorations`, and `App::set_window_move_area` for custom client-side title bars.
