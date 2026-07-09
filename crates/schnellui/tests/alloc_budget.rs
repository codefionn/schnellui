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
//! shape SOUL §4.1 prescribes. `frame()` is the **CPU-side** signal→paint loop:
//! pull the signal graph (`Runtime::flush`), route the change to ONE node's paint
//! property, and flag that node paint-dirty ready for a byte
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
        .child(
            Text::dynamic_into(move |b| {
                let _ = std::fmt::Write::write_fmt(b, format_args!("{}", count.get()));
            })
            .role(schnellui::a11y::Role::Status),
        )
        .child(Button::new("increment").on_click(move || count.set(count.get() + 1)))
        .gap(8.0)
}

/// The reviewed **`text_edit`** allocation budget (SOUL §4.1). It was 3 allocs / 23 bytes
/// (the producer `String`, the change-suppression `last.clone()`, and the a11y `value`
/// clone). The producer now writes into the slot's reused buffer, the comparison with
/// the last value is by borrow, the buffers swap on change, and the a11y and text
/// layout strings are rewritten in place, so the warm path (count "81" -> "18": same
/// width, both digits rasterized, pooled Parley context warm) allocates **0**. A
/// first-seen glyph still costs a one-time atlas insert, kept out of this steady-warm
/// number by permuting already-rasterized digits. Budgets only go down.
const TEXT_EDIT_BUDGET: usize = 0;

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
        .child(
            Text::dynamic_into(move |b| {
                let _ = std::fmt::Write::write_fmt(b, format_args!("{}", count.get()));
            })
            .role(schnellui::a11y::Role::Status),
        )
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

    // Prove the counting allocator is live, so the zero below is a real result.
    let probe = allocation_counter::measure(|| {
        std::hint::black_box(Vec::<u8>::with_capacity(std::hint::black_box(8)));
    });
    assert!(probe.count_total >= 1, "counting allocator is not live");
    assert!(
        info.count_total <= TEXT_EDIT_BUDGET as u64,
        "text_edit re-render allocated {} times / {} bytes (budget {TEXT_EDIT_BUDGET}) — SOUL §4.1",
        info.count_total,
        info.bytes_total
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

#[test]
fn many_clean_text_areas_allocate_nothing() {
    use schnellui::widgets::{Scroll, TextArea};
    let mut inner = Column::new();
    for _ in 0..6 {
        inner = inner.child(TextArea::new("alpha\nbeta\ngamma"));
    }
    let mut view = Column::new().child(Scroll::new().child(inner));
    for _ in 0..6 {
        view = view.child(TextArea::new("one\ntwo"));
    }
    let mut app = App::mount_with_size(view, 320, 480);
    app.frame();
    app.frame();

    let info = allocation_counter::measure(|| app.frame());
    assert_eq!(
        info.count_total, 0,
        "clean frame over 12 text areas must reuse the id scratch"
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

/// A host-driven terminal for the patched-path gates: `rows` lines of 80 columns.
/// Each revision bump rewrites row 3 with one of two strings that use the same
/// glyphs in a different order, so the measured update reuses the glyph cache.
struct PatchedTerminal {
    app: App,
    revision: std::rc::Rc<std::cell::Cell<u64>>,
    id: schnellui::scene::WidgetId,
}

const ROW_A: &str = "user@host:~/src/schnellui$ cargo test -p schnellui --features count";
const ROW_B: &str = "tnuoc serutaef-- iullenhcs p- tset ograc $iullenhcs/crs/~:tsoh@resu";

fn patched_terminal(rows: usize) -> PatchedTerminal {
    use schnellui::widgets::{TerminalCellMetrics, TerminalGridPatch};
    let revision = std::rc::Rc::new(std::cell::Cell::new(0_u64));
    let source = revision.clone();
    let host = revision.clone();
    let grid = TerminalGrid::patched(
        move || source.get(),
        move |patch: &mut TerminalGridPatch<'_>| {
            patch.resize(80, rows);
            let text = if host.get().is_multiple_of(2) {
                ROW_A
            } else {
                ROW_B
            };
            for (cell, ch) in patch.row_mut(3).iter_mut().zip(text.char_indices()) {
                // Inline graphemes: the assignment neither allocates nor frees.
                cell.grapheme = text[ch.0..ch.0 + ch.1.len_utf8()].into();
            }
        },
    )
    .metrics(TerminalCellMetrics::new(8.0, 16.0, 13.0))
    .label("Shell");
    let mut app = App::mount_with_size(grid, 640, (rows * 16) as u32);
    let id = app
        .find_widget(Role::ScrollView, Some("Shell"))
        .expect("terminal mounted");
    app.frame();
    PatchedTerminal { app, revision, id }
}

impl PatchedTerminal {
    fn bump(&mut self) {
        self.revision.set(self.revision.get() + 1);
        self.app.frame();
    }
}

/// `TerminalGrid::patched` paint gate: after warm-up, a host update that rewrites
/// one row's cells (graphemes already in the glyph cache) plus `frame()` performs
/// zero allocations. This covers the patch call, the in-place a11y value splice
/// (`sync_a11y_value`), the one-row paint rebuild and the scene range splice. The
/// AccessKit `TreeUpdate` a platform adapter builds afterwards is measured
/// separately below because it is foreign storage (SOUL §6.2).
#[test]
fn patched_terminal_row_rewrite_allocates_nothing() {
    let mut term = patched_terminal(24);
    for _ in 0..4 {
        term.bump();
    }
    for _ in 0..4 {
        let info = allocation_counter::measure(|| term.bump());
        assert_eq!(
            info.count_total, 0,
            "one-row patched terminal update must not allocate (SOUL §1)"
        );
        assert_eq!(info.bytes_total, 0);
    }
    // Eight bumps: the last patch wrote ROW_A, and the frame polled it with no
    // signal involved (the path a `redraw_signal()` wake takes).
    let value = term
        .app
        .scene()
        .a11y(term.id)
        .unwrap()
        .value
        .clone()
        .unwrap();
    assert_eq!(value.lines().count(), 24);
    assert!(value.lines().nth(3).unwrap().starts_with(ROW_A));
}

/// The reviewed AccessKit row for a terminal text change (SOUL §6.2). Building the
/// one-node `TreeUpdate` copies the terminal's accessible value into AccessKit's
/// own `Node` storage, so it allocates a fixed number of times per changed node.
/// The count does not depend on the grid size; the bytes are the length of the
/// value (about rows x columns). Measured 4 allocs / 2,244 bytes at 24 rows and
/// 4 / 19,740 at 240 rows. Budgets only go down.
const TERMINAL_A11Y_UPDATE_BUDGET: u64 = 4;

#[test]
fn patched_terminal_a11y_update_stays_within_budget() {
    let mut counts = Vec::new();
    for rows in [24, 240] {
        let mut term = patched_terminal(rows);
        for _ in 0..4 {
            term.bump();
        }
        term.app
            .scene_mut()
            .mark_dirty(term.id, schnellui::scene::DirtyFlags::A11Y);
        let _ = term.app.a11y_tree_update();
        let info = allocation_counter::measure(|| {
            std::hint::black_box(term.app.a11y_tree_update());
        });
        eprintln!(
            "terminal a11y TreeUpdate ({rows} rows): {} allocs / {} bytes",
            info.count_total, info.bytes_total
        );
        assert!(
            info.count_total <= TERMINAL_A11Y_UPDATE_BUDGET,
            "terminal a11y update allocated {} times (budget {TERMINAL_A11Y_UPDATE_BUDGET})",
            info.count_total
        );
        counts.push(info.count_total);
    }
    assert_eq!(
        counts[0], counts[1],
        "allocation count must not scale with rows"
    );
}

/// Cloning or `clone_from` of a typical terminal cell never allocates: the
/// grapheme is inline and the hyperlink is a shared `Arc<str>`.
#[test]
fn terminal_cell_clone_allocates_nothing() {
    use schnellui::widgets::TerminalCell;
    let mut cell = TerminalCell::new("é", Color::WHITE, Color::BLACK);
    cell.hyperlink = Some(std::sync::Arc::from("https://example.test/a/long/path"));
    let mut target = TerminalCell::new("界", Color::BLACK, Color::WHITE);
    let info = allocation_counter::measure(|| {
        let copy = std::hint::black_box(cell.clone());
        target.clone_from(&copy);
        target.grapheme = "x".into();
    });
    assert_eq!(info.count_total, 0);
    assert_eq!(info.bytes_total, 0);
}

/// Key dispatch to a focused terminal-like surface looks its binding up by
/// borrowing the focused node's accessible name, so routing allocates nothing.
#[test]
fn focused_key_dispatch_allocates_nothing() {
    use schnellui::raw_keyboard::{Key, KeyCode, KeyLocation, NamedKey, PhysicalKey};
    use schnellui::{
        FocusedInputEvent, FocusedInputResult, RawInputState, RawKeyEvent, RawModifiers,
    };
    let mut term = patched_terminal(4);
    let id = term.id;
    term.app
        .register_focused_input_handler(Role::ScrollView, Some("Shell"), |_| {
            FocusedInputResult::Handled
        });
    assert!(term.app.focus(Some(id)));
    term.app.frame();
    let dispatch = |app: &mut App| {
        app.dispatch_focused_input(FocusedInputEvent::Key(RawKeyEvent {
            logical_key: Key::Named(NamedKey::Enter),
            physical_key: PhysicalKey::Code(KeyCode::Enter),
            key_without_modifiers: Key::Named(NamedKey::Enter),
            location: KeyLocation::Standard,
            modifiers: RawModifiers::default(),
            state: RawInputState::Pressed,
            repeat: false,
            text: None,
            text_with_all_modifiers: None,
        }))
    };
    assert_eq!(dispatch(&mut term.app), FocusedInputResult::Handled);
    let info = allocation_counter::measure(|| {
        std::hint::black_box(dispatch(&mut term.app));
    });
    assert_eq!(info.count_total, 0, "focused key routing must not allocate");
    assert_eq!(info.bytes_total, 0);
}

/// A browser panel streams same-size RGBA frames through
/// `Image::dynamic_rgba_versioned`. Each new frame overwrites the image's existing
/// atlas texels in place (`ImageAtlas::write_rect`, a sub-rect GPU upload) with no
/// remount and no allocation inside the frame.
#[test]
fn streamed_image_frame_allocates_nothing() {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    let revision = Rc::new(Cell::new(0_u64));
    let frames: [Rc<[u8]>; 2] = [
        vec![0x10; 64 * 32 * 4].into(),
        vec![0xf0; 64 * 32 * 4].into(),
    ];
    let current = Rc::new(RefCell::new(DynamicImageFrame::new(
        64,
        32,
        frames[0].clone(),
    )));
    let source = revision.clone();
    let feed = current.clone();
    let view =
        Image::dynamic_rgba_versioned(move || source.get(), move || Some(feed.borrow().clone()));
    let mut app = App::mount_with_size(view, 320, 240);
    app.frame();
    let atlas_size = (app.scene().images().width(), app.scene().images().height());
    for step in 1..=8_u64 {
        let next = DynamicImageFrame::new(64, 32, frames[step as usize % 2].clone());
        let info = allocation_counter::measure(|| {
            *current.borrow_mut() = next;
            revision.set(step);
            app.frame();
        });
        if step > 1 {
            assert_eq!(info.count_total, 0, "streamed frame {step} allocated");
            assert_eq!(info.bytes_total, 0);
        }
    }
    assert_eq!(
        (app.scene().images().width(), app.scene().images().height()),
        atlas_size,
        "same-size frames reuse their atlas region"
    );
}
