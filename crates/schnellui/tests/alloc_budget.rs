//! # The allocation covenant, made executable (SOUL §1, §4.1)
//!
//! > "a signal changing repaints the screen without allocating a single byte on the
//! > heap" — SOUL §0.
//!
//! This file hosts `rerender_on_one_signal_allocates_nothing` — *the test that
//! defines us* (SOUL §1). It measures the **second** `set_signal + frame` invocation
//! (never the first — a warm-capacity false-pass must not sneak through, §4.1) and
//! asserts `count_total == 0 && bytes_total == 0` on the steady-state re-render path.
//!
//! ## What is measured, and what is honestly excluded
//!
//! The measured closure is `app.set_signal("count", n); app.frame();` — the literal
//! shape SOUL §4.1 prescribes. `frame()` is the **CPU-side** signal→damage loop:
//! pull the signal graph (`Runtime::flush`), route the change to ONE node's paint
//! property, and fold that node's rect into the frame damage region ready for a byte
//! upload (§3.2). It deliberately does **not** submit to the GPU — `render_to_png`
//! owns that, and wgpu's own submission internals allocate inside a foreign crate we
//! do not control (§4.1 explicitly permits splitting the measurement to the CPU-side
//! path when GPU submission allocates). So this asserts the covenant over exactly the
//! path the covenant is about: *decide what changed, prepare the delta, zero heap
//! traffic.*
//!
//! The scenario drives a **reactive paint binding** (`App::bind_paint`) — a signal
//! mapped to one node's fill colour. That is the pure `rerender_1_signal` row of the
//! `ALLOC_BUDGETS` table (§4.1), which is *literal zero* by law. A counter whose
//! signal changes **text** is a different row — `text_edit` — which §4.1 lists as a
//! *small, budgeted, non-zero* cost (integer→`String` formatting, new-glyph atlas
//! inserts, a one-node AccessKit `TreeUpdate` whose foreign `Vec`/`String` storage we
//! cannot pool, §6.2). `text_edit_stays_within_budget` measures that real
//! through-`App` counter path — the dynamic count re-shaped through the pooled Parley
//! context into fresh glyph quads — and asserts it stays within a small reviewed
//! budget, so the text path is measured too — just not held to literal zero, exactly
//! as the soul document prescribes.
#![cfg(feature = "count-allocations")]

use schnellui::a11y::Role;
use schnellui::layout::{Align, Container, ContainerStyle};
use schnellui::scene::{Color, Primitive, Rect, WidgetKind};
use schnellui::signal::create_signal;
use schnellui::widgets::{
    Button, Column, DynamicImageFrame, Image, TerminalGrid, TerminalGridModel, Text, View, WrapMode,
};
use schnellui::{App, TestValue};

/// Builds a headless app whose single node's fill colour is a signal-bound reactive
/// paint binding — the pure `rerender_1_signal` path (SOUL §4.1). `set_signal("count",
/// n)` advances the signal; `frame()` pulls it and repaints that one node.
fn paint_scenario(width: u32, height: u32) -> App {
    let mut app = App::new(width, height);
    let node = app.scene_mut().insert(WidgetKind::Button, None);
    app.scene_mut().set_root(node);
    app.scene_mut()
        .set_rect(node, Rect::new(0.0, 0.0, width as f32, height as f32));
    // Seed a primitive so `set_color` has a fill to mutate (mount may allocate, §4).
    app.scene_mut().replace_primitives(
        node,
        [Primitive::SolidRect {
            rect: Rect::new(0.0, 0.0, width as f32, height as f32),
            color: Color::BLACK,
            corner_radius: 0.0,
        }],
    );

    let count = create_signal(0i64);
    app.register_signal("count", move |v| {
        if let TestValue::Int(i) = v {
            count.set(i);
        }
    });
    // signal -> one node's fill colour (Copy in, Copy out ⇒ zero-alloc eval, §4.1)
    app.bind_paint(node, move || {
        let n = count.get();
        Color::rgb((n & 0xFF) as u8, ((n >> 8) & 0xFF) as u8, 0)
    });
    app
}

/// **The test that defines us** (SOUL §1). Mount, warm, then measure the *second*
/// `set_signal + frame` — assert zero allocs and zero bytes on the steady-state
/// re-render path.
#[test]
fn rerender_on_one_signal_allocates_nothing() {
    let mut app = paint_scenario(400, 200);

    // First mount + frame is allowed to allocate (grow caches & pools, §4.1).
    app.frame();
    // Warm the *second-invocation* capacity so a first-time grow can't false-pass.
    app.set_signal("count", 1);
    app.frame();

    let info = allocation_counter::measure(|| {
        app.set_signal("count", std::hint::black_box(2));
        app.frame();
    });

    assert_eq!(
        info.count_total, 0,
        "steady-state re-render must perform 0 allocations (SOUL §1)"
    );
    assert_eq!(
        info.bytes_total, 0,
        "steady-state re-render must allocate 0 bytes (SOUL §1)"
    );
}

/// A second, independent signal-change frame is still zero — proves the first
/// measured frame did not merely ride warm capacity that a third frame would exhaust.
#[test]
fn repeated_rerenders_stay_zero() {
    let mut app = paint_scenario(320, 240);
    app.frame();
    app.set_signal("count", 7);
    app.frame();

    for v in [11i64, 22, 33, 44] {
        let info = allocation_counter::measure(|| {
            app.set_signal("count", std::hint::black_box(v));
            app.frame();
        });
        assert_eq!(info.count_total, 0, "alloc on rerender to {v}");
        assert_eq!(info.bytes_total, 0, "bytes on rerender to {v}");
    }
}

/// The counter UI, constructed at `start` (mirror of the example's `view!` chain).
fn counter_view(start: i64) -> impl View {
    let count = create_signal(start);
    Column::new()
        .child(Text::new("Counter"))
        .child(Text::dynamic(move || count.get().to_string()).role(schnellui::a11y::Role::Status))
        .child(Button::new("increment").on_click(move || count.set(count.get() + 1)))
        .gap(8.0)
}

/// The reviewed **`text_edit`** allocation budget (SOUL §4.1). Picked by measurement:
/// the warm re-shape path (see [`text_edit_stays_within_budget`]) allocates exactly
/// **3** times / 23 bytes, reproducibly. The path is *not* literal zero — §4.1
/// explicitly budgets it — but it must be small and stable; a regression that
/// ballooned it fails the test. Raising this number is a reviewed diff with a
/// justification, never a drive-by (SOUL §4.1).
///
/// The three budgeted allocations, on the measured warm path (count "81" → "18": same
/// width ⇒ no relayout, both digits already rasterized, pooled Parley context warm):
///   1. the producer's `i64 -> String` format (`count.get().to_string()`),
///   2. the change-suppression `last.clone()` in `run_dynamic_slots`,
///   3. the a11y `value` `String` (`Role::Status` announces the new value, §6.2).
///
/// Parley's warm re-shape runs entirely through the pooled `LayoutContext`/`Layout`
/// and contributes **0** (amortized zero, §4.1). A first-seen glyph would additionally
/// cost a one-time atlas insert — still a §4.1 text_edit grow-event, deliberately kept
/// out of this steady-warm number by permuting already-rasterized digits.
const TEXT_EDIT_BUDGET: usize = 3;

/// The **`text_edit`** budget row (SOUL §4.1): the real through-`App` counter, whose
/// signal change re-formats an integer to a `String`, re-shapes the label through the
/// pooled context, re-emits its glyph quads, and flags a one-node a11y `TreeUpdate`.
/// This is the sibling of the literal-zero `rerender_1_signal` soul test — it proves
/// the *text* path is measured and bounded, just not zero (SOUL §4.1). We measure the
/// **second** warm invocation with the width held stable (no relayout) and the digits
/// already rasterized, so the number is reproducible.
#[test]
fn text_edit_stays_within_budget() {
    // Register a driveable "count" bound to the same signal the view closes over.
    let count = create_signal(0i64);
    let view = Column::new()
        .child(Text::new("Counter"))
        .child(Text::dynamic(move || count.get().to_string()).role(schnellui::a11y::Role::Status))
        .child(Button::new("increment").on_click(move || count.set(count.get() + 1)));
    let mut app = App::mount_with_size(view, 400, 200);
    app.register_signal("count", move |v| {
        if let TestValue::Int(i) = v {
            count.set(i);
        }
    });

    // Warm: build + rasterize the digits we will permute, grow every pool/capacity,
    // and settle the 2nd-invocation state so no first-time grow can false-pass.
    app.frame();
    app.set_signal("count", 18); // rasterizes '1' and '8'
    app.frame();
    app.set_signal("count", 81); // same width, digits cached, pools warm
    app.frame();

    // Measure the *second* warm re-shape: "81" -> "18" (changed, same width ⇒ no
    // relayout, no new glyph) — the reproducible steady-warm text_edit cost.
    let info = allocation_counter::measure(|| {
        app.set_signal("count", std::hint::black_box(18));
        app.frame();
    });

    // The text path MUST allocate (integer→String) — asserting `>= 1` doubles as proof
    // the counting allocator is genuinely live, so the literal-zero assertions in the
    // pure-path soul test are real results, not a dead counter.
    assert!(
        info.count_total >= 1,
        "text_edit path unexpectedly allocated nothing — is the counting allocator live?"
    );
    // Small, reviewed, reproducible ceiling (SOUL §4.1 — measured, not zero).
    assert!(
        info.count_total <= TEXT_EDIT_BUDGET as u64,
        "text_edit re-render allocated {} times (budget {TEXT_EDIT_BUDGET}) — SOUL §4.1",
        info.count_total
    );
    // silence unused warning on the standalone helper in non-panicking runs
    let _ = counter_view(0);
}

const PARAGRAPH: &str = "The quick brown fox jumps over the lazy dog again and again";

/// A stretch column so a wrapping text child wraps to the viewport (SOUL §8.1).
fn stretch_column(child: impl View) -> Column {
    let mut style = ContainerStyle::new(Container::Column);
    style.align = Align::Stretch;
    Column::new().style(style).child(child)
}

/// **Covenant guard (SOUL §1):** a steady-state re-render with a **wrapping text node
/// present but unchanged** must still be *literal zero* allocations. The wrapping path
/// is deferred/width-aware, but a clean frame never enters it: the global-version gate
/// skips `run_dynamic_slots` (no dynamic slots — the paragraph is static) and the
/// layout block is skipped (nothing layout-dirty), so `emit_wrapped_paint` is never
/// called. The only work is the reactive paint binding on a sibling button — the pure
/// `rerender_1_signal` path. Measured on the second warm invocation.
#[test]
fn wrapped_text_present_rerender_allocates_nothing() {
    let count = create_signal(0i64);
    // A static wrapped paragraph (deferred-paint, width-aware) sits beside a button
    // whose fill is a signal-bound paint binding.
    let view = stretch_column(
        Column::new()
            .child(Text::new(PARAGRAPH).wrap(WrapMode::Word))
            .child(Button::new("go")),
    );
    let mut app = App::mount_with_size(view, 300, 240);
    let btn = app
        .find_widget(Role::Button, Some("go"))
        .expect("button present");
    app.bind_paint(btn, move || {
        let n = count.get();
        Color::rgb((n & 0xFF) as u8, ((n >> 8) & 0xFF) as u8, 0)
    });
    app.register_signal("count", move |v| {
        if let TestValue::Int(i) = v {
            count.set(i);
        }
    });

    // Warm: mount + first frame lay out & emit wrapped glyphs (grow event, §4.1),
    // then a second frame to settle 2nd-invocation capacity.
    app.frame();
    app.set_signal("count", 1);
    app.frame();

    let info = allocation_counter::measure(|| {
        app.set_signal("count", std::hint::black_box(2));
        app.frame();
    });
    assert_eq!(
        info.count_total, 0,
        "steady-state re-render with wrapping text present must be zero-alloc (SOUL §1)"
    );
    assert_eq!(info.bytes_total, 0, "…and zero bytes (SOUL §1)");
}

/// **Covenant guard (SOUL §1) for the rich text surface:** a steady-state re-render
/// with a **rich document viewer and a multi-line editor present but unchanged**
/// must still be *literal zero* allocations. Both are deferred-paint widgets, but a
/// clean frame never enters their passes: the global-version gate skips
/// `run_dynamic_slots` (the document is static, so no dynamic sites), the layout
/// block is skipped (nothing layout-dirty), so neither `emit_rich_paint` nor the
/// area re-emit pass runs, and their width/rect idempotence gates never even fire.
/// The only work is the reactive paint binding on a sibling button — the pure
/// `rerender_1_signal` path. Measured on the second warm invocation.
#[test]
fn rich_text_present_rerender_allocates_nothing() {
    use schnellui::widgets::{RichDoc, RichSpan, RichText, TextArea};
    let count = create_signal(0i64);
    let doc = RichDoc::new()
        .heading(1, ["Doc"])
        .paragraph([
            RichSpan::plain("body with "),
            RichSpan::bold("emphasis"),
            RichSpan::plain(" and "),
            RichSpan::code("code"),
        ])
        .code_block("rust", [vec![RichSpan::code("fn main() {}")]])
        .quote(["quoted"])
        .rule();
    let view = stretch_column(
        Column::new()
            .child(RichText::new(doc))
            .child(TextArea::new("line one\nline two"))
            .child(Button::new("go")),
    );
    let mut app = App::mount_with_size(view, 360, 480);
    let btn = app
        .find_widget(Role::Button, Some("go"))
        .expect("button present");
    app.bind_paint(btn, move || {
        let n = count.get();
        Color::rgb((n & 0xFF) as u8, ((n >> 8) & 0xFF) as u8, 0)
    });
    app.register_signal("count", move |v| {
        if let TestValue::Int(i) = v {
            count.set(i);
        }
    });

    // Warm: mount + first frame flow the document and emit its glyphs (grow
    // event, §4.1), then a second frame settles 2nd-invocation capacity.
    app.frame();
    app.set_signal("count", 1);
    app.frame();

    let info = allocation_counter::measure(|| {
        app.set_signal("count", std::hint::black_box(2));
        app.frame();
    });
    assert_eq!(
        info.count_total, 0,
        "steady-state re-render with rich text + text area present must be zero-alloc (SOUL §1)"
    );
    assert_eq!(info.bytes_total, 0, "…and zero bytes (SOUL §1)");
}

/// Embedded workbenches can show more than four live browser/canvas surfaces at
/// once. Polling unchanged versioned frames must retain its key-snapshot storage
/// instead of spilling a fresh small vector on every frame.
#[test]
fn many_clean_dynamic_images_allocate_nothing() {
    let pixels: std::rc::Rc<[u8]> = vec![0xff, 0xff, 0xff, 0xff].into();
    let mut view = Column::new();
    for _ in 0..6 {
        let pixels = pixels.clone();
        view = view.child(Image::dynamic_rgba_versioned(
            || 0,
            move || Some(DynamicImageFrame::new(1, 1, pixels.clone())),
        ));
    }
    let mut app = App::mount_with_size(view, 320, 240);
    app.frame();
    app.frame();

    let info = allocation_counter::measure(|| app.frame());
    assert_eq!(
        info.count_total, 0,
        "clean frame over six dynamic images must reuse polling scratch"
    );
    assert_eq!(info.bytes_total, 0);
}

/// Multi-pane workbenches likewise retain more terminal grids than the former
/// four-item inline snapshot. Clean grids should be an allocation-free no-op.
#[test]
fn many_clean_terminal_grids_allocate_nothing() {
    let mut view = Column::new();
    for _ in 0..6 {
        view = view.child(TerminalGrid::new(TerminalGridModel::new(1, 1)));
    }
    let mut app = App::mount_with_size(view, 320, 240);
    app.frame();
    app.frame();

    let info = allocation_counter::measure(|| app.frame());
    assert_eq!(
        info.count_total, 0,
        "clean frame over six terminals must reuse polling scratch"
    );
    assert_eq!(info.bytes_total, 0);
}

/// A **wrap-affecting change** is *not* zero — it is layout-dirty work (re-measure,
/// re-wrap, re-emit multi-line glyphs, **and a full Taffy relayout**) which SOUL §4.1
/// budgets as a small, reviewed, non-zero cost (the `text_edit` + `resize` families
/// combined). This test **measures and pins** that number so a regression that
/// balloons it fails. It drives a dynamic wrapped text between two paragraphs on the
/// warm path (glyphs cached, pools warm) and asserts the count stays within a small
/// reviewed ceiling.
///
/// **Measured: 24 allocs / ~15.3 KB** on the warm second change. The bulk is the
/// coarse relayout (SOUL §4.1 `resize` row — `sync_tree` re-mirrors the subtree into
/// Taffy and Taffy's own layout caches allocate) plus the two shapes the width-aware
/// path performs (one to *measure* the height, one to *emit* the multi-line glyphs).
/// The ceiling carries headroom over the measured figure so it is not flaky across
/// std/toolchain versions; raising it further is a reviewed diff (SOUL §4.1).
const WRAP_CHANGE_BUDGET: usize = 40;

#[test]
fn wrap_change_stays_within_budget() {
    let alt = "A slightly different paragraph of words wrapping across several lines here";
    let toggle = create_signal(false);
    let view = stretch_column(
        Text::dynamic(move || {
            if toggle.get() {
                alt.to_string()
            } else {
                PARAGRAPH.to_string()
            }
        })
        .wrap(WrapMode::Word),
    );
    let mut app = App::mount_with_size(view, 180, 400);
    app.register_signal("t", move |v| {
        if let TestValue::Bool(b) = v {
            toggle.set(b);
        }
    });

    // Warm both strings so their glyphs are rasterized and every pool is grown.
    app.frame();
    app.set_signal("t", true);
    app.frame();
    app.set_signal("t", false);
    app.frame();

    // Measure the second warm wrap-affecting change.
    let info = allocation_counter::measure(|| {
        app.set_signal("t", std::hint::black_box(true));
        app.frame();
    });
    eprintln!(
        "wrap-change allocation: {} allocs / {} bytes",
        info.count_total, info.bytes_total
    );
    assert!(
        info.count_total >= 1,
        "wrap change must allocate (producer String etc.) — is the counter live?"
    );
    assert!(
        info.count_total <= WRAP_CHANGE_BUDGET as u64,
        "wrap-change allocated {} times (budget {WRAP_CHANGE_BUDGET}) — SOUL §4.1",
        info.count_total
    );
}
