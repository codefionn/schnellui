# Reactivity & State Management

`schnellui` has no virtual DOM and never re-runs component functions on state changes. Instead, state flows through a fine-grained reactive graph (`schnellui::signal`) and an optional structured store facade (`schnellui::store`), updating only the retained nodes that depend on changed data.

---

## 1. Signals, Memos, Effects & Scopes (`schnellui::signal`)

All reactive nodes live in a `Send + Sync` generational arena (`SlotMap`). Handles (`Signal<T>`, `Memo<T>`, `Effect`, `Scope`) are lightweight `Copy` keys — you can capture them in multiple `move ||` closures without `Rc`/`Arc` cloning boilerplate or reference-count churn.

### Signals

A `Signal<T>` holds a reactive value (`T: Send + Sync + 'static`):

```rust
use schnellui::signal::create_signal;

let count = create_signal(0_i64);

// Tracked reads (subscribe the currently running memo, effect, or dynamic UI slot)
let current = count.get();                 // clones T; panics if disposed
let maybe = count.try_get();               // Option<T>; None if disposed
let doubled = count.with(|n| n * 2);       // borrows &T without cloning

// Untracked reads (do not record a dependency)
let snapshot = count.peek(|n| *n);         // alias: read_untracked
let maybe_snap = count.try_peek(|n| *n);   // alias: try_read_untracked

// Writes (push phase: marks dependents dirty, runs no recomputation yet)
count.set(10);
count.update(|n| *n += 1);
let old = count.transaction(|n| std::mem::replace(n, 0));
```

#### Opt-in Equality Gates

By default, `create_signal` fires dependents on every `set()`, even if the new value equals the old one. To suppress no-op notifications, create a signal with an equality comparator:

```rust
use std::any::Any;
use schnellui::signal::create_signal_equals;

fn eq_i64(a: &dyn Any, b: &dyn Any) -> bool {
    a.downcast_ref::<i64>() == b.downcast_ref::<i64>()
}

let count = create_signal_equals(0_i64, eq_i64);
count.set(0); // equal -> no version bump, no downstream work
```

> **Floating-point note:** Under `PartialEq`, `f32::NAN != f32::NAN`, so an equality-gated signal holding `NaN` would treat every `set(NaN)` as a change. Use bitwise comparison (`a.to_bits() == b.to_bits()`) for floats if `NaN` can occur.

### Memos (Derived State)

A `Memo<T>` caches a derived computation (`T: Send + Sync + Clone + 'static`). It evaluates lazily on demand and is **glitch-free on diamonds** — if `source` feeds both `memo_a` and `memo_b`, and `join` reads both, `join` recomputes once with consistent values:

```rust
use schnellui::signal::{create_memo, create_signal};

let celsius = create_signal(20.0_f64);
let fahrenheit = create_memo(move || celsius.get() * 9.0 / 5.0 + 32.0);

assert_eq!(fahrenheit.get(), 68.0);
celsius.set(100.0);
assert_eq!(fahrenheit.get(), 212.0);
```

Like signals, memos default to propagating whenever they recompute. Use `create_memo_equals(compute, eq_fn)` to prune downstream propagation when the derived result did not change.

### Effects & Flushing

An `Effect` runs once synchronously on creation, tracks every signal/memo it reads, and queues itself to re-run whenever a tracked dependency changes:

```rust
use schnellui::signal::{create_effect, create_signal, Runtime};

let name = create_signal(String::from("Ada"));
let _effect = create_effect(move || {
    name.with(|n| println!("Hello, {n}"));
});

name.set(String::from("Grace"));
Runtime::flush(); // runs queued effects (App::frame() calls this automatically)
```

- **Push phase (`set` / `update`)** only marks direct observers `Dirty`, deeper descendants `Check`, and queues affected effects/subscriptions. No user closures run during `set`.
- **Pull phase (`Runtime::flush` or `Memo::get`)** settles dirty nodes in dependency order. `App::frame()` flushes the runtime automatically.
- **Lock safety:** The runtime never holds its arena lock while running user closures (`with`, `update`, memos, effects), so closures can freely read or write other signals without deadlocking.

### Ownership Scopes

Group signals, memos, and effects under a `Scope` so they can be torn down together:

```rust
use schnellui::signal::{create_scope, create_signal, run_in_scope};

let scope = create_scope();
let value = run_in_scope(scope, || create_signal(42));
assert_eq!(value.try_get(), Some(42));

scope.dispose();
assert_eq!(value.try_get(), None);
```

---

## 2. Shared Application State (`schnellui::store`)

When an application has a structured state model with many fields, putting everything into one `Signal<AppState>` would wake every subscriber whenever any field changes. `Store<T>` solves this by pairing a revision signal with a `RecursiveRwLock<T>` payload and equality-gated `Selector<U>` projections:

```rust
use schnellui::store::Store;

#[derive(Clone)]
struct AppState {
    title: String,
    clicks: usize,
}

let store = Store::new(AppState {
    title: "Draft".into(),
    clicks: 0,
});

// Each selector is an equality-gated Memo:
// changing `clicks` will NOT wake consumers of `title`.
let title = store.select(|s| s.title.clone());
let clicks = store.select(|s| s.clicks);

store.update(|s| s.clicks += 1);
assert_eq!(clicks.get(), 1);

// Return values from in-place mutations without cloning the whole state:
let prev = store.transaction(|s| std::mem::replace(&mut s.title, "Published".into()));
assert_eq!(prev, "Draft");
assert_eq!(title.get(), "Published");
```

Key properties of `Store` and `Selector`:

- **RAII cleanup:** Each `Selector` owns its own child `Scope` via `Arc`. Dropping the last clone of a selector disposes its memo automatically; dropping the last `Store` and `Selector` handle disposes the store's scope.
- **Nested reads:** `store.read(...)` / `store.with(...)` use a recursive read lock, so nested reads of the same store cannot deadlock even if a writer thread is queued. (Never call `set`/`update`/`transaction` from inside a `read`/`with` closure on the same store.)
- **Untracked reads:** Use `store.peek(...)` (`read_untracked`) to read payload fields after subscribing to a narrower `Selector`.

---

## 3. Binding State to the UI

`schnellui` offers four layers of UI updates, ordered from cheapest (in-place mutation) to most structural (replacing nodes):

### A. Dynamic Text Slots

Inside `view!`, any `(expr)` child compiles to `Text::dynamic_into(...)`, which formats the value into a reused `String` buffer via `std::fmt::Write`:

```rust
let count = create_signal(0_i64);

view! {
    column {
        text { "Static heading" }              // hoisted; never re-evaluated
        text { (count.get()) }                 // dynamic slot; updates in place
        text { (format!("Items: {}", count.get())) }
    }
}
```

Or with the builder API directly (zero-allocation when the formatted string fits in the reused buffer and keeps the same measured width):

```rust
use std::fmt::Write;
use schnellui::widgets::Text;

let label = Text::dynamic_into(move |buf| {
    let _ = write!(buf, "{}", count.get());
});
```

Only slots whose tracked signals actually changed are evaluated during `App::frame()`. Unchanged text leaves paint, layout, and accessibility completely clean.

### B. Reactive Attributes

In `view!`, passing a closure to a supported attribute creates a tracked `ReactiveAttr` binding (`View::reactive_attr`) that mutates the retained node in place and skips work when the value is unchanged:

```rust
use schnellui::scene::Color;
use schnellui::signal::create_signal;
use schnellui::view;

let active = create_signal(false);
let label_color = create_signal(Color::BLACK);

let ui = view! {
    column(gap = move || if active.get() { 16.0 } else { 8.0 }) {
        text(
            color = move || label_color.get(),
            size = move || if active.get() { 20.0 } else { 16.0 },
        ) { "Status" }
        checkbox(checked = move || active.get())
        button(disabled = move || !active.get()) { "Continue" }
    }
};
```

Supported reactive attributes (`schnellui::widgets::Attribute`):

- **Paint / Visual**: `color`, `size` / `font_size`
- **Layout**: `width`, `height`, `min_width`, `min_height`, `max_width`, `max_height`, `grow`, `shrink`, `basis`, `gap`, `padding` / `all`
- **Widget State**: `disabled`, `checked`, `value` (slider position via `View::reactive_attr`), `invalid` (validation state via `View::reactive_attr`)

> Notice how disabling a currently focused control automatically hands keyboard focus off to the nearest enabled sibling in tab order, preserving focus-ring visibility rules.

### C. Paint Bindings (`App::bind_paint`)

For hot visual properties that only change a retained node's color (`Copy` in, `Copy` out), `App::bind_paint` updates the node's fill through `Scene::set_color` with **literal zero heap allocations** and no relayout:

```rust
app.bind_paint(widget_id, move || {
    if active.get() { Color::rgb(0x2e, 0x63, 0xd4) } else { Color::rgb(0x7a, 0x86, 0x99) }
});
```

### D. Structural Updates: Subtree Replacement & Remounts

When the **shape** of the tree changes (adding/removing list items, switching routes, opening/closing panels), you choose between targeted subtree replacement and a full window remount:

#### 1. Targeted Subtree Replacement (Preferred for Local Changes)

Attach a stable `ComponentRef` to a branch, then replace only that branch while preserving the rest of the scene, sibling focus, scroll positions, text engine, and GPU surface:

```rust
use schnellui::scene::ComponentRef;
use schnellui::widgets::{Column, Text, View as _};
use schnellui::{SubtreeReplacement, WindowUpdate};

let content_ref = ComponentRef::new();

// In your initial view:
let root = Column::new()
    .child(Text::new("Persistent header"))
    .child(build_content().with_ref(content_ref));

// Direct API:
app.replace_subtree(content_ref, build_next_content())?;

// Or from a windowed update hook (`App::run_windowed_with_updates*`):
// return Some(WindowUpdate::subtree(
//     SubtreeReplacement::new(content_ref, build_next_content(), "tab_switched")
//         .with_focus_after(some_ref),
// ));
```

In `view!`, attach a ref with `component_ref(value = content_ref) { column { ... } }`.

`SubtreeReplacement` also supports:
- `.with_focus_after(target_ref)` — Move focus to a retained sibling after replacing a transient control.
- `.with_focus_ring()` — Show the keyboard focus ring on that target.
- `.with_escape_return()` / `.with_escape_fallback(target_ref)` — Mark a replaced branch as a docked region so pressing Escape returns focus to the control that opened it.

#### 2. Full Windowed Remounts

For route transitions or full-workspace rebuilds, use the windowed update hooks (`App::run_windowed_with`, `App::run_windowed_with_viewport_reasoned_remount`, or `App::run_windowed_with_updates*`) and wrap replacement construction in `App::with_remount_engine` so the replacement inherits the running text shaper and glyph atlas without re-rasterizing glyphs:

```rust
use schnellui::{App, Remount};

app.run_windowed_with_viewport_reasoned_remount("My App", move |viewport| {
    pending_route.take().map(|route| {
        let next = App::with_remount_engine(|| build_app(route, viewport));
        Remount::new(next, "route_changed")
    })
})?;
```

Remounts automatically transfer interactive widget state (focus, scroll offsets, text carets, open context menus, active drags, and modal opener focus) to matching controls in the new tree.

#### 3. Large Dynamic Lists Without Full Remounts

For long or streaming lists (thousands of rows), use `VirtualList` + `VirtualListController<K>` (`schnellui::widgets`), which mounts only the visible viewport plus overscan and reconciles rows incrementally by key without rebuilding the surrounding tree.

---

## 4. Dependency Injection (`Context`)

Pass shared handles (stores, themes, localization catalogs, services) down a tree explicitly via `Context`:

```rust
use schnellui::widgets::Context;
use schnellui::App;

#[derive(Clone)]
struct ApiBase(String);

let ctx = Context::new().provide(ApiBase("https://example.invalid".into()));
let app = App::mount_with_context(ctx, |ctx| {
    let api = ctx.get::<ApiBase>().unwrap();
    build_root(&api.0)
});
```

Child views can derive scoped contexts with `ctx.with(override_value)` without mutating sibling scopes.

---

## 5. Staying Zero-Allocation on the Hot Path

`SOUL.md` requires **0 heap allocations, 0 reallocations, and 0 frees** on steady-state re-renders. Here is how to write reactive UI that honors this:

- **Prefer `Copy` values in hot signals** (`i64`, `f32`, `bool`, `Color`, small enums). `Signal::set` reuses the existing boxed slot in place.
- **Format into buffers, not fresh `String`s.** Use `view!`'s `(expr)` slot (which lowers to `Text::dynamic_into` + `write!`) rather than `format!(...)` inside a dynamic closure.
- **Use `Store::select` for structured state** so unrelated fields don't wake widgets.
- **Use `TerminalGrid::patched` and `Image::dynamic_rgba_versioned`** for high-frequency streaming terminals and video/browser frames.
- **Know what is budgeted vs zero:**
  - *Literal zero:* signal push/pull, clean frames, `bind_paint`, same-width warm dynamic text edits, direct scroll offsets, caret movement, hover transitions, patched terminal rows, same-size streamed image frames, and CPU rasterization of steady-state re-renders.
  - *Small fixed budget:* first mount, window resize / text re-wrap, new glyphs entering the atlas, and AccessKit `TreeUpdate` construction (which allocates inside the foreign `accesskit` crate).
