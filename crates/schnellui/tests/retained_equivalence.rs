//! # Retained == reconstructed (SOUL §3.2) — the multi-frame equivalence oracle
//!
//! The whole GPU-first claim (SOUL §3.2) is that a *retained* scene driven through an
//! event sequence renders **pixel-identically** to a scene *freshly constructed* in
//! that same end state. A one-shot screenshotter (SOUL §7) only ever renders a
//! *constructed* state, so it cannot catch a bug that only appears once a frame has
//! already run and a later event mutates the retained tree — exactly the class the
//! windowed event loop exercises.
//!
//! These tests replay the interactive sequence **headlessly, without a window**:
//! mount → `frame` → click (via the same `dispatch_click` handler the windowed
//! pointer path fires, SOUL §6.3) → `frame` → … → render, then assert the retained
//! app's pixels equal a freshly-mounted app in the identical end state.
//!
//! ## The bug they pin (fragment anchoring, SOUL §8.1)
//!
//! `run_dynamic_slots` re-emits a dynamic text's glyph quads at a *provisional* local
//! origin and relied on the frame's `reposition_paint` pass to slide them onto the
//! laid-out origin — but that pass only runs when the frame is **layout-dirty**. A
//! value-only change whose shaped **width is unchanged** (e.g. "0 of 3" → "1 of 3",
//! or a single-digit counter tick) never flags layout, so the re-emitted glyphs stay
//! stranded at the origin, rendering on top of the title at the top-left. Driven
//! *headless* scenarios dodged it only because they dispatch every click **before**
//! the first `frame` (so the one relayout anchors everything); the corruption needs a
//! frame to have already run, then a click. These tests reproduce that ordering.
//!
//! Each test skips gracefully (not fails) when no GPU adapter is present (SOUL §7.2).

use schnellui::a11y::Role;
use schnellui::layout::{Align, Container, ContainerStyle};
use schnellui::scene::Primitive;
use schnellui::signal::{create_memo, create_signal};
use schnellui::widgets::{Button, Checkbox, Column, Text, View, WrapMode};
use schnellui::App;

/// A stretch column that makes its single child fill the viewport width, so a wrapping
/// text child wraps to the viewport and re-wraps on resize (SOUL §8.1).
fn stretch_column(child: impl View) -> Column {
    let mut style = ContainerStyle::new(Container::Column);
    style.align = Align::Stretch;
    Column::new().style(style).child(child)
}

/// Collects every node's paint primitives in pre-order as position-rounded tuples
/// (`tag, x, y, w, h` ×10), so two independently-built scenes can be compared for
/// **structural + positional** equivalence *without a GPU* (SOUL §3.2). This is the
/// headless guard for the fragment-anchoring / wrap-caching bug class: a stranded or
/// mis-wrapped glyph shifts these tuples even when no adapter is present to render.
fn glyph_layout(app: &App) -> Vec<(u8, i32, i32, i32, i32)> {
    fn r(v: f32) -> i32 {
        (v * 10.0).round() as i32
    }
    let mut out = Vec::new();
    let scene = app.scene();
    let mut stack = match scene.root() {
        Some(root) => vec![root],
        None => vec![],
    };
    // pre-order, children in declared order
    while let Some(id) = stack.pop() {
        if let Some(pd) = scene.paint(id) {
            for p in &pd.primitives {
                let t = match p {
                    Primitive::SolidRect { rect, .. } => (0u8, rect),
                    Primitive::GlyphQuad { rect, .. } => (1u8, rect),
                    Primitive::ImageQuad { rect, .. } => (2u8, rect),
                    Primitive::Line { .. } => continue,
                };
                out.push((t.0, r(t.1.x), r(t.1.y), r(t.1.width), r(t.1.height)));
            }
        }
        if let Some(node) = scene.node(id) {
            for &c in node.children.iter().rev() {
                stack.push(c);
            }
        }
    }
    out
}

/// The counter UI (mirror of the `counter` example's builder chain): a static title,
/// one dynamic `Status` text bound to the count, and an increment button.
fn counter_view(start: i64) -> impl View {
    let count = create_signal(start);
    Column::new()
        .child(Text::new("Counter"))
        .child(Text::dynamic(move || count.get().to_string()).role(Role::Status))
        .child(Button::new("increment").on_click(move || count.set(count.get() + 1)))
        .gap(8.0)
}

/// The settings UI (mirror of the `settings` example): a static title, a **memo**-
/// derived `Status` summary ("N of 3 enabled") over three checkbox-backed signals,
/// and the three checkboxes. Reproduces the reported windowed corruption: one
/// checkbox click updates the memo, which re-emits the summary text.
fn settings_view(initial: [bool; 3]) -> impl View {
    let a = create_signal(initial[0]);
    let b = create_signal(initial[1]);
    let c = create_signal(initial[2]);
    // A 3-source → 1-derived memo, exactly like the example's live summary.
    let summary = create_memo(move || {
        let n = [a, b, c].into_iter().filter(|s| s.get()).count();
        format!("{n} of 3 enabled")
    });
    Column::new()
        .child(Text::new("Settings").size(22.0))
        .child(Text::dynamic(move || summary.get()).role(Role::Status))
        .child(Checkbox::new(initial[0]).on_toggle(move |v| a.set(v)))
        .child(Checkbox::new(initial[1]).on_toggle(move |v| b.set(v)))
        .child(Checkbox::new(initial[2]).on_toggle(move |v| c.set(v)))
        .gap(6.0)
}

/// Asserts two tightly-packed RGBA8 buffers are byte-identical, with a differing-pixel
/// count on failure (SOUL §3.2 — retained == reconstructed is exact, not perceptual).
fn assert_pixel_identical(label: &str, retained: &[u8], reconstructed: &[u8]) {
    assert_eq!(
        retained.len(),
        reconstructed.len(),
        "{label}: image byte-lengths differ ({} vs {}) — a resize/extent mismatch",
        retained.len(),
        reconstructed.len(),
    );
    if retained == reconstructed {
        return;
    }
    let mut diff_px = 0usize;
    for i in (0..retained.len()).step_by(4) {
        if retained[i..i + 4] != reconstructed[i..i + 4] {
            diff_px += 1;
        }
    }
    panic!(
        "{label}: retained render != reconstructed render — {diff_px} of {} pixels differ \
         (fragment-anchoring regression, SOUL §3.2/§8.1)",
        retained.len() / 4
    );
}

/// The reported no-resize case: a single checkbox click updates a memo-derived summary
/// across an already-rendered frame. `frame` → click → `frame`. The summary
/// "0 of 3" → "1 of 3" is **same width**, so the frame is not layout-dirty and the
/// pre-fix code stranded the re-emitted glyphs at the origin.
#[test]
fn retained_equals_reconstructed_after_checkbox_click_no_resize() {
    // Retained: mount defaults, render a frame FIRST, THEN click and frame again.
    let mut retained = App::mount_with_size(settings_view([false, false, false]), 360, 220);
    retained.frame();
    let cb = retained
        .find_widget(Role::CheckBox, None)
        .expect("first checkbox present");
    retained.dispatch_click(cb); // the exact windowed pointer path (§6.3)
    retained.frame();
    let Some(bytes_retained) = retained.render_rgba8() else {
        eprintln!(
            "skipping retained_equals_reconstructed_after_checkbox_click_no_resize: no GPU adapter"
        );
        return;
    };

    // Reconstructed: mount directly in the end state (dark mode already on).
    let mut reconstructed = App::mount_with_size(settings_view([true, false, false]), 360, 220);
    reconstructed.frame();
    let bytes_reconstructed = reconstructed
        .render_rgba8()
        .expect("adapter was present for the retained render above");

    assert_pixel_identical(
        "settings checkbox click (no resize)",
        &bytes_retained,
        &bytes_reconstructed,
    );
}

/// The counter's frame-then-click ordering with a **same-width** single-digit tick
/// ("0" → "1"): `frame` → click → `frame`. Not layout-dirty, so it isolates the
/// anchoring bug from any relayout that would have masked it.
#[test]
fn retained_equals_reconstructed_counter_frame_then_click() {
    let mut retained = App::mount_with_size(counter_view(0), 400, 200);
    retained.frame(); // frame 1 anchors "0" via reposition_paint
    let inc = retained
        .find_widget(Role::Button, Some("increment"))
        .expect("increment button present");
    retained.dispatch_click(inc); // 0 -> 1
    retained.frame(); // re-emit "1"; same width ⇒ frame is not layout-dirty
    let Some(bytes_retained) = retained.render_rgba8() else {
        eprintln!(
            "skipping retained_equals_reconstructed_counter_frame_then_click: no GPU adapter"
        );
        return;
    };

    let mut reconstructed = App::mount_with_size(counter_view(1), 400, 200);
    reconstructed.frame();
    let bytes_reconstructed = reconstructed
        .render_rgba8()
        .expect("adapter was present for the retained render above");

    assert_pixel_identical(
        "counter frame-then-click (same width)",
        &bytes_retained,
        &bytes_reconstructed,
    );
}

/// A **changed-width** tick ("9" → "10") drives a genuine relayout, so this guards
/// that the anchoring fix stays correct on the layout-dirty path too (it must not
/// double-apply the slide).
#[test]
fn retained_equals_reconstructed_counter_changed_width() {
    let mut retained = App::mount_with_size(counter_view(9), 400, 200);
    retained.frame();
    let inc = retained
        .find_widget(Role::Button, Some("increment"))
        .expect("increment button present");
    retained.dispatch_click(inc); // 9 -> 10 (width grows)
    retained.frame();
    let Some(bytes_retained) = retained.render_rgba8() else {
        eprintln!("skipping retained_equals_reconstructed_counter_changed_width: no GPU adapter");
        return;
    };

    let mut reconstructed = App::mount_with_size(counter_view(10), 400, 200);
    reconstructed.frame();
    let bytes_reconstructed = reconstructed
        .render_rgba8()
        .expect("adapter was present for the retained render above");

    assert_pixel_identical(
        "counter changed-width tick",
        &bytes_retained,
        &bytes_reconstructed,
    );
}

/// The full interactive sequence from the brief, including a window resize:
/// mount → `frame` → click → `frame` → `resize` → `frame` → click → `frame`. Compared
/// against a fresh app mounted directly in the end state at the resized dimensions —
/// exercising both the fragment-anchoring path and the headless resize/extent path
/// (SOUL §8).
#[test]
fn retained_equals_reconstructed_full_interactive_sequence_with_resize() {
    let mut retained = App::mount_with_size(counter_view(0), 400, 200);
    retained.frame();
    let inc = retained
        .find_widget(Role::Button, Some("increment"))
        .expect("increment button present");
    retained.dispatch_click(inc); // -> 1
    retained.frame();
    retained.resize(600.0, 320.0); // a grow event (SOUL §4 resize row)
    retained.frame();
    let inc = retained
        .find_widget(Role::Button, Some("increment"))
        .expect("increment button present after resize");
    retained.dispatch_click(inc); // -> 2
    retained.frame();
    let Some(bytes_retained) = retained.render_rgba8() else {
        eprintln!("skipping retained_equals_reconstructed_full_interactive_sequence_with_resize: no GPU adapter");
        return;
    };

    // Reconstructed directly in the end state: count 2, at the resized dimensions.
    let mut reconstructed = App::mount_with_size(counter_view(2), 600, 320);
    reconstructed.frame();
    let bytes_reconstructed = reconstructed
        .render_rgba8()
        .expect("adapter was present for the retained render above");

    assert_pixel_identical(
        "full interactive sequence + resize",
        &bytes_retained,
        &bytes_reconstructed,
    );
}

const PARAGRAPH: &str = "The quick brown fox jumps over the lazy dog again and again";

/// (a) A **wrapped paragraph** rendered wide, then resized narrower (which re-wraps to
/// more lines), must match a fresh app mounted directly at the narrow size — retained
/// re-wrap == reconstructed (SOUL §3.2/§8.1). The headless `glyph_layout` guard runs
/// with no adapter; the pixel oracle runs when one is present.
#[test]
fn retained_equals_reconstructed_wrapped_paragraph_resize() {
    let mut retained = App::mount_with_size(
        stretch_column(Text::new(PARAGRAPH).wrap(WrapMode::Word)),
        400,
        300,
    );
    retained.frame(); // wraps at 400
    retained.resize(180.0, 300.0); // narrower -> more lines, re-measure + re-emit
    retained.frame();

    let reconstructed_view = stretch_column(Text::new(PARAGRAPH).wrap(WrapMode::Word));
    let mut reconstructed = App::mount_with_size(reconstructed_view, 180, 300);
    reconstructed.frame();

    // Headless structural guard (no GPU needed): the re-wrapped glyph positions must
    // equal the freshly-wrapped ones exactly.
    assert_eq!(
        glyph_layout(&retained),
        glyph_layout(&reconstructed),
        "wrapped paragraph: retained re-wrap glyph layout != reconstructed at 180px"
    );

    // Pixel oracle when an adapter is available.
    let Some(bytes_retained) = retained.render_rgba8() else {
        eprintln!("skipping wrapped_paragraph_resize pixel check: no GPU adapter");
        return;
    };
    let bytes_reconstructed = reconstructed
        .render_rgba8()
        .expect("adapter was present for the retained render above");
    assert_pixel_identical(
        "wrapped paragraph resize",
        &bytes_retained,
        &bytes_reconstructed,
    );
}

/// (b) A **dynamic wrapped text** whose signal change alters the line count: mount
/// short (1 line), `frame`, flip the signal to a long paragraph (multiple lines),
/// `frame` (re-wrap + re-emit + relayout), and compare against a fresh app mounted
/// directly in the long end state (SOUL §3.2/§8.1). Guards that a wrapped dynamic
/// re-emit re-wraps and re-anchors exactly like a fresh construction.
#[test]
fn retained_equals_reconstructed_dynamic_wrapped_line_count_change() {
    let toggle = create_signal(false); // false = short
    let view = stretch_column(
        Text::dynamic(move || {
            if toggle.get() {
                PARAGRAPH.to_string()
            } else {
                "short".to_string()
            }
        })
        .wrap(WrapMode::Word),
    );
    let mut retained = App::mount_with_size(view, 180, 300);
    retained.frame(); // short: one line
    toggle.set(true); // -> long paragraph: many lines
    retained.frame(); // re-wrap, relayout, re-emit

    // Reconstructed: mount directly in the long end state.
    let long = create_signal(true);
    let recon_view = stretch_column(
        Text::dynamic(move || {
            if long.get() {
                PARAGRAPH.to_string()
            } else {
                "short".to_string()
            }
        })
        .wrap(WrapMode::Word),
    );
    let mut reconstructed = App::mount_with_size(recon_view, 180, 300);
    reconstructed.frame();

    assert_eq!(
        glyph_layout(&retained),
        glyph_layout(&reconstructed),
        "dynamic wrapped: retained re-wrap glyph layout != reconstructed"
    );

    let Some(bytes_retained) = retained.render_rgba8() else {
        eprintln!("skipping dynamic_wrapped_line_count_change pixel check: no GPU adapter");
        return;
    };
    let bytes_reconstructed = reconstructed
        .render_rgba8()
        .expect("adapter was present for the retained render above");
    assert_pixel_identical(
        "dynamic wrapped line-count change",
        &bytes_retained,
        &bytes_reconstructed,
    );
}
