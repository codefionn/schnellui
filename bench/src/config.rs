use crate::workloads::*;

// # schnellui-bench — the allocation covenant, made runnable (SOUL §4)
//
// A CLI that measures **wall time** and **heap allocations** per named hot path and
// judges each against the SOUL §4.1 `ALLOC_BUDGETS` table. It is the generalisation
// of `crates/schnellui/tests/alloc_budget.rs` into a tool: the same closures the soul
// test asserts on, but timed, iterated, tabulated, and turned into a CI gate.
//
// ```text
// cargo run -p schnellui-bench                 # both tables, exits nonzero on a breach
// cargo run -p schnellui-bench -- --json       # machine-readable (incl. per-n series)
// cargo run -p schnellui-bench -- --list       # path names, one per line
// cargo run -p schnellui-bench -- --filter rerender --iters 5000
// ```
//
// ## Two tables
//
// 1. **Budget table** — one row per registered path, judged against its §4.1 budget.
// 2. **Proportionality table** — the headline claim of SOUL Directive #3 ("a signal
//    change costs work proportional to what changed — not to the size of the tree")
//    made executable: `rerender_1`, `text_edit`, and `frame_clean` are re-measured
//    over retained documents of n = 10 / 100 / 1000 rows. The median-time
//    ratio (n=1000 over n=10) must stay under [`FLAT_RATIO`] and the per-iteration
//    alloc counts must be *identical* across n, or the run fails.
//
// ## What is (and is not) measured — read before trusting a number
//
// Every measured closure is **CPU-side only**. `App::frame()` with no renderer
// attached (the bench never calls `render_to_png`) runs exactly the CPU half of the
// signal→damage loop (SOUL §3.2, §8.1): `Runtime::flush` pulls the signal graph,
// reactive paint bindings + dynamic-text slots mutate the retained node(s) in place,
// layout runs only if a measured width moved, and each changed node is marked
// in `scene.paint_dirty()` ready for a byte upload. It **does not** submit to the GPU —
// wgpu's submission internals allocate inside a foreign crate we do not control, so
// §4.1 explicitly permits splitting the measurement to the CPU-side path. So these
// numbers cover *decide-what-changed + prepare-the-delta*, never the GPU upload.
//
// ## Targeted dynamic-text delivery (the polling floor, closed)
//
// `schnellui::widgets::run_dynamic_slots` drains a per-app ready queue populated by
// tracked signal subscriptions (SOUL §3.1). A clean frame evaluates no producers,
// and a signal write reaches only dynamic sites that read that signal; unrelated
// sites are not polled. A clean frame over a `Text::dynamic` site therefore costs
// **zero** allocations (the `dyn_text_poll_clean` row). The large-document rows
// still use the covenant's Copy signal→color path as their changed dynamic site.
//
// ## Allocation counting, honestly
//
// `allocation-counter` installs a counting `#[global_allocator]` and exposes
// `count_total` (every allocation), `bytes_total`, and `count_current` (the net
// alloc−free balance inside the window). It does **not** override `realloc`, so the
// trait's default `realloc = alloc + copy + dealloc` splits every reallocation into
// one counted `alloc` (bumping `count_total` and `bytes_total`) plus one `dealloc`.
// There is therefore **no separate realloc counter** — a growing `Vec` shows up as an
// extra `count_total` with `count_current` still netting to zero. The `ALLOCS`
// column is `count_total` (allocations **including** reallocations); the covenant's
// `allocs + reallocs + frees == 0` is gated as `count_total == 0 && bytes_total == 0
// && count_current == 0` (the last term catches a lone free of a pre-window buffer).
//
// ## Measurement hygiene (SOUL §4.1)
//
// - **Second-and-later invocations only.** Each path builds + warms its state, then
//   warms the steady-state closure a further [`WARMUP`] times, so a first-mount
//   warm-capacity grow can never false-pass.
// - **Timing and counting are separate passes** over the same closure instance —
//   counting perturbs timing, so they never share a loop.
// - **Steady means steady.** For every zero-budget path the per-iteration alloc count
//   must be *identical* across all iterations; a drifting count fails the gate.
// - **Mount-class rows run at most [`MOUNT_ITERS_CAP`] iterations** — each iteration
//   is a full tree build + shape of every paragraph, so thousands of samples of a
//   grow event would only slow CI without sharpening a number that is Report-only.

/// Extra warmup invocations of the steady-state closure before either measured pass,
/// so we always measure the *second-and-later* invocation (SOUL §4.1).
pub(crate) const WARMUP: usize = 8;

/// Iteration cap for mount-class rows (each iteration is a full build; a grow event
/// needs few samples and n=200..1000 builds are milliseconds each).
pub(crate) const MOUNT_ITERS_CAP: u64 = 25;

/// The document sizes (paragraph counts) of the proportionality series.
pub(crate) const SCALE_NS: [usize; 3] = [10, 100, 1000];

/// SCALES-FLAT iff median-time(n=1000) / median-time(n=10) < this. Chosen generous:
/// a walk linear in document size would show ~100x between n=10 and n=1000, and even
/// a sqrt(n) walk ~10x, so 3x cleanly separates "flat" from any real scaling while
/// absorbing CI noise, cache-locality drift from the larger retained scene, and
/// scheduler jitter. Tightening it is a reviewed diff (SOUL §4.1 spirit).
pub(crate) const FLAT_RATIO: f64 = 3.0;

// `text_edit` is literal zero (was 3 allocs / 23 bytes): the dynamic text producer
// writes into the slot's reused buffer, the last value is compared by borrow and the
// buffers swap, and the a11y / text layout strings are rewritten in place. The same
// holds at every document size (Directive #3).

/// Paragraph count of the wrapped-document point rows (`large_wrapped_mount`,
/// `wrapped_rerender_1`, `wrapped_frame_clean`) — the n=200 sample matching the
/// unwrapped `large_text_*` rows so the wrapping cost is read off against them.
pub(crate) const WRAPPED_N: usize = 200;

/// Logical viewport width the wrapped docs mount at. Deliberately **narrow** so each
/// ~54-char paragraph (see [`paragraph`]) actually breaks across multiple lines under
/// `WrapMode::Word` — otherwise a wide viewport would leave every paragraph single-line
/// and the "wrapped" rows would measure the legacy path. 300px wraps the stock
/// sentences to ~2 lines (the same width `tests/alloc_budget.rs` wraps `PARAGRAPH` at).
pub(crate) const WRAPPED_WIDTH: u32 = 300;
pub(crate) const WRAPPED_HEIGHT: u32 = 600;

/// The two viewport widths `wrap_reflow` toggles between (resize-style). Both wrap
/// [`REFLOW_PARAGRAPH`] to a *different* line count, so each toggle is a genuine
/// re-wrap + reflow (not a no-op relayout).
pub(crate) const REFLOW_NARROW: f32 = 180.0;
pub(crate) const REFLOW_WIDE: f32 = 260.0;

/// A single paragraph that wraps to multiple lines at BOTH [`REFLOW_NARROW`] and
/// [`REFLOW_WIDE`] (so every toggle changes the wrap), matching the string
/// `tests/alloc_budget.rs::wrap_change_stays_within_budget` pins its budget on.
pub(crate) const REFLOW_PARAGRAPH: &str =
    "The quick brown fox jumps over the lazy dog again and again";

/// The `wrap_reflow` budget — a wrap-affecting change is layout-dirty work, not zero.
///
/// **Cited number (INTEGRATE report + `tests/alloc_budget.rs::WRAP_CHANGE_BUDGET`):**
/// the reported wrap-change was **MEASURED at 24 allocs / 15 345 bytes** on the warm
/// second change — the coarse full-tree Taffy relayout (the `resize` family: `sync_tree`
/// re-mirrors the subtree into Taffy, whose own layout caches allocate) plus the two
/// shapes the width-aware path performs (one to *measure* the wrapped height, one to
/// *emit* the multi-line glyphs). That reviewed ceiling is pinned at **40** allocs to
/// carry headroom over the deterministic 24 across std/toolchain versions; we mirror it
/// verbatim here. The byte ceiling carries matching (~1.6x) headroom over the measured
/// 15 345. Raising either is a reviewed diff (SOUL §4.1).
pub(crate) const WRAP_REFLOW_ALLOCS: u64 = 40;
pub(crate) const WRAP_REFLOW_BYTES: u64 = 24_576;

/// Ceiling for `text_reflow` and `ellipsis_resize`: a width change on a ~700 glyph
/// text. Measured 9 allocs / 5 016 bytes and 11 / 5 112 (was 18 / 54 744 and
/// 68 / 121 374 when measure and paint shaped separately into fresh buffers and
/// truncation re-shaped prefixes). The remaining allocations are the Taffy relayout
/// of `App::resize`, not text.
pub(crate) const LONG_TEXT_RESIZE_ALLOCS: u64 = 16;
pub(crate) const LONG_TEXT_RESIZE_BYTES: u64 = 8_192;

/// Lines in the `textarea_*` rows' document.
pub(crate) const TEXTAREA_LINES: usize = 2000;

// ---------------------------------------------------------------------------
// Budgets — one row per path, mirroring ALLOC_BUDGETS (SOUL §4.1).
// ---------------------------------------------------------------------------

/// A path's allocation budget (SOUL §4.1 `ALLOC_BUDGETS`).
#[derive(Clone, Copy)]
pub(crate) enum Budget {
    /// **Literal zero is the law** (SOUL §1): `count_total == 0 && bytes_total == 0`
    /// and no frees (`count_current == 0`), *identical* across every iteration. A
    /// breach fails the gate (nonzero exit) — this is the soul made executable.
    Zero,
    /// A small, fixed, **reviewed** ceiling (SOUL §4.1): the worst iteration must
    /// satisfy `allocs <= allocs && bytes <= bytes`. Raising it is a reviewed diff.
    Bounded { allocs: u64, bytes: u64 },
    /// A grow event by definition (SOUL §4 — first-mount may allocate). Reported, not
    /// gated: we print the number, we never fail on it.
    Report,
}

impl Budget {
    /// The compact `BUDGET` column string.
    pub(crate) fn label(self) -> String {
        match self {
            Budget::Zero => "0 / 0B".to_string(),
            Budget::Bounded { allocs, bytes } => format!("<={allocs} / <={bytes}B"),
            Budget::Report => "report".to_string(),
        }
    }
}

/// A named benchmark path: a factory that builds + warms its state and hands back the
/// steady-state closure to measure (SOUL §4.1 one row per path).
pub(crate) struct BenchPath {
    pub(crate) name: &'static str,
    pub(crate) budget: Budget,
    /// one-line note printed under the table (what the row covers / excludes).
    pub(crate) note: &'static str,
    /// iteration cap (mount-class rows), applied as `min(--iters, cap)`.
    pub(crate) iters_cap: Option<u64>,
    pub(crate) make: fn() -> Box<dyn FnMut()>,
}

/// The registered paths, mirroring `ALLOC_BUDGETS` (SOUL §4.1). Order = report order.
pub(crate) fn paths() -> Vec<BenchPath> {
    vec![
        BenchPath {
            name: "signal_set_flush",
            budget: Budget::Zero,
            note: "pure reactive graph: 1 signal -> 1 memo -> 1 effect; set + Runtime::flush, no UI. Edge lists + flush scratch are persistent (SOUL §3.1) => steady-state zero.",
            iters_cap: None,
            make: make_signal_set_flush,
        },
        BenchPath {
            name: "memo_diamond",
            budget: Budget::Zero,
            note: "diamond: source -> {memo_a, memo_b} -> join; set + pull(join.get()). Glitch-free demand-driven pull recomputes each node once, in place (SOUL §3.1) => zero.",
            iters_cap: None,
            make: make_memo_diamond,
        },
        BenchPath {
            name: "signal_broadcast_1000",
            budget: Budget::Zero,
            note: "one signal read by 1,000 effects; set + Runtime::flush re-runs all of them. Each re-run reuses its single edge slot (SOUL §3.1 epoch reuse), so this is 1,000 effect runs with zero allocation; scale_broadcast_per_observer gates that the per-observer cost is flat.",
            iters_cap: None,
            make: make_signal_broadcast_1000,
        },
        BenchPath {
            name: "mount",
            budget: Budget::Report,
            note: "App::mount_with_size_scaled of a counter-like UI. A first-mount grow event (SOUL §4) — allowed to allocate; the number is reported, never gated. Iters capped at 25 (mount-class).",
            iters_cap: Some(MOUNT_ITERS_CAP),
            make: make_mount,
        },
        BenchPath {
            name: "theme_transition_frame",
            budget: Budget::Report,
            note: "one theme-transition frame over a 200-paragraph themed app (apply_color_scheme with zero duration => the rebuild_with_theme every animation frame runs). The shaper and glyph atlas are carried into the rebuilt app, so no glyph is re-rasterized. Mount-class: reported, never gated. Iters capped at 25.",
            iters_cap: Some(MOUNT_ITERS_CAP),
            make: make_theme_rebuild_frame,
        },
        BenchPath {
            name: "remount_state_transfer_1000",
            budget: Budget::Report,
            note: "indexed structural-remount restoration over 1,000 same-label editors. Each scene is traversed once and explicit refs reserve their targets; this guards scriptschnellng's large workbench replacement seam against the former editor_count × tree_size walk.",
            iters_cap: Some(MOUNT_ITERS_CAP),
            make: make_large_form_remount_state,
        },
        BenchPath {
            name: "chat_generation_subtree_200_in_1200",
            budget: Budget::Report,
            note: "rebuild + layout a 200-row transcript branch inside a retained 1,200-row app; shell, composer-equivalent siblings, text engine, atlases, and runtime stay resident.",
            iters_cap: Some(MOUNT_ITERS_CAP),
            make: make_chat_generation_subtree,
        },
        BenchPath {
            name: "chat_generation_full_remount_1200",
            budget: Budget::Report,
            note: "comparison row: rebuild + layout the complete 1,200-row app for the same transcript update.",
            iters_cap: Some(MOUNT_ITERS_CAP),
            make: make_chat_generation_full_remount,
        },
        BenchPath {
            name: "rerender_1_signal",
            budget: Budget::Zero,
            note: "THE soul path (SOUL §1): warm frame, then set_signal + frame over one reactive paint binding (Copy in, Copy out). Literal zero by law — a breach fails the run.",
            iters_cap: None,
            make: make_rerender_1_signal,
        },
        BenchPath {
            name: "rerender_n_signals",
            budget: Budget::Zero,
            note: "8 independent bound signals all set before ONE frame. Work is proportional to changed nodes, all Copy => still literal zero (SOUL §1, §3).",
            iters_cap: None,
            make: make_rerender_n_signals,
        },
        BenchPath {
            // Budget cited from crates/schnellui/tests/alloc_budget.rs::TEXT_EDIT_BUDGET
            // (currently 0 allocs / 0 bytes).
            name: "text_edit",
            budget: Budget::Zero,
            note: "dynamic text digit swap 18<->81 (same width, warm glyphs) + frame. The producer writes into a reused buffer, so the warm path is literal zero (alloc_budget.rs TEXT_EDIT_BUDGET = 0).",
            iters_cap: None,
            make: make_text_edit,
        },
        BenchPath {
            name: "frame_clean",
            budget: Budget::Zero,
            note: "a frame with nothing dirty: signal effects and retained subscriptions both have empty ready queues (SOUL §3.1); layout is skipped and no binding writes occur.",
            iters_cap: None,
            make: make_frame_clean,
        },
        BenchPath {
            name: "scroll_direct_long",
            budget: Budget::Zero,
            note: "App::dispatch_scroll to one retained ScrollView in a 1,000-row real text document, then retire the CPU paint/a11y delta. The offset and pre-capacitated a11y value mutate in place; layout is structurally asserted clean. GPU gather/upload is deliberately unmeasured.",
            iters_cap: None,
            make: make_scroll_direct_long,
        },
        BenchPath {
            name: "wheel_route_long",
            budget: Budget::Report,
            note: "App::dispatch_wheel_at over the same 1,000-row document: indexed modal/viewport lookup + innermost scroll routing + retained mutation. The scale_wheel_route row gates document-size independence; clipping and modal correctness are covered by widget tests.",
            iters_cap: None,
            make: make_wheel_route_long,
        },
        BenchPath {
            name: "scroll_debounce_due",
            budget: Budget::Zero,
            note: "real App schedule + due-fire cycle for a debounced scroll callback. The callback is taken/restored with Option<Box<_>>, so no dummy callback allocation is permitted; layout remains structurally clean.",
            iters_cap: None,
            make: make_scroll_debounce_due,
        },
        BenchPath {
            name: "hover_move_between_buttons",
            budget: Budget::Zero,
            note: "pointer moves between two buttons of a 1,000-row document while a third wears the focus ring, then a frame. Only the old and new hover decorations change hands; the ring and the other rows are untouched. The scale_hover_move row gates document-size independence.",
            iters_cap: None,
            make: make_hover_move_between_buttons,
        },
        BenchPath {
            name: "virtual_list_append",
            budget: Budget::Report,
            note: "append one row to a 10,000-row VirtualList and run the frame that reconciles it (streaming transcript). The positions map and prefix index grow in place; the window is unchanged. The scale_virtual_list_append row gates source-length independence over 1k/10k/100k rows.",
            iters_cap: None,
            make: make_virtual_list_append,
        },
        BenchPath {
            name: "terminal_patch_row",
            budget: Budget::Zero,
            note: "TerminalGrid::patched, 80x24: the host rewrites one row's cells (glyphs cached) when its revision moves, then frame. The patch writes the retained model in place; the frame re-emits that one row, splices the scene range and splices the row's text into the a11y value. No model clone, no grid diff. The AccessKit TreeUpdate is outside frame() (alloc_budget.rs TERMINAL_A11Y_UPDATE_BUDGET). The scale_terminal_patch_row row gates grid-height independence.",
            iters_cap: None,
            make: make_terminal_patch_row,
        },
        BenchPath {
            name: "browser_frame_stream",
            budget: Budget::Zero,
            note: "Image::dynamic_rgba_versioned shows a 640x360 RGBA frame stream: a new same-size frame + frame overwrites the image's atlas region in place (ImageAtlas::write_rect, logged as a sub-rect upload). No remount. The 900 KB memcpy into the atlas mirror dominates the time.",
            iters_cap: None,
            make: make_browser_frame_stream,
        },
        // ---- large-document rows (SOUL Directive #3: work ∝ change, not size) ----
        BenchPath {
            name: "large_text_mount",
            budget: Budget::Report,
            note: "mount of the n=200 doc: ~11k glyph instances over ~70 DISTINCT glyphs at 16px (instances share atlas entries per (glyph,size)), so the 1024x1024 atlas holds the coverage with room to spare. Grow event — reported, never gated. Iters capped at 25 (mount-class).",
            iters_cap: Some(MOUNT_ITERS_CAP),
            make: make_large_text_mount,
        },
        BenchPath {
            name: "large_text_rerender_1",
            budget: Budget::Zero,
            note: "n=200 doc, dynamic site = ONE signal->color paint binding mid-doc (the covenant's Copy path). set + frame must stay literal zero INDEPENDENT of document size (SOUL §1 + Directive #3).",
            iters_cap: None,
            make: make_large_text_rerender_1,
        },
        BenchPath {
            name: "large_text_edit",
            budget: Budget::Zero,
            note: "digit swap 18<->81 in the ONE Text::dynamic mid the n=200 doc (same width, warm glyphs). Same zero budget as the small text_edit — the text path must not cost more in a big document (Directive #3).",
            iters_cap: None,
            make: make_large_text_edit,
        },
        BenchPath {
            name: "large_text_frame_clean",
            budget: Budget::Zero,
            note: "clean frame over the retained 200-paragraph doc (paint-binding variant). Empty targeted-ready queues must not care how much retained text sits behind them (SOUL §3.1, Directive #3).",
            iters_cap: None,
            make: make_large_text_frame_clean,
        },
        BenchPath {
            name: "dyn_text_poll_clean",
            budget: Budget::Zero,
            note: "clean frame over the n=200 doc WITH its Text::dynamic present and its signal UNCHANGED. The targeted subscription queue is empty, so no producer String or `last` clone is created; clean delivery remains literal zero allocation (SOUL §3.1, §3.3).",
            iters_cap: None,
            make: make_dyn_text_poll_clean,
        },
        // ---- wrapped-document rows (SOUL §8.1 text wrap; Directive #3 still holds) ----
        BenchPath {
            name: "large_wrapped_mount",
            budget: Budget::Report,
            note: "mount of the n=200 doc as WRAPPED paragraphs (WrapMode::Word in a stretch column at 300px => each ~54-char paragraph breaks to ~2 lines). Same lifecycle boundary as large_text_mount: App::mount, NO frame. The wrapped BUILD only registers set_fill_width + the deferred TextLayout config — it does NOT shape: a wrapping leaf's glyphs are shaped width-aware in the first frame's emit_wrapped_paint (unmeasured here), whereas large_text_mount's build shapes every paragraph inline (emit_text_paint). So this figure is LOWER than large_text_mount precisely because wrapped shaping is deferred out of the build, not because wrapping is cheaper overall. Grow event — reported, never gated. Iters capped at 25 (mount-class).",
            iters_cap: Some(MOUNT_ITERS_CAP),
            make: make_large_wrapped_mount,
        },
        BenchPath {
            name: "wrapped_rerender_1",
            budget: Budget::Zero,
            note: "THE soul path (SOUL §1) with WRAPPED text retained: n=200 wrapped paragraphs around ONE signal->color paint binding (a Button mid-doc). A paint-only change keeps layout clean, so emit_wrapped_paint never runs and the static wrapped leaves never enter run_dynamic_slots (slots empty => version-gate early-return). Copy in, Copy out => literal zero, INDEPENDENT of the retained wrapped text (mirrors alloc_budget.rs::wrapped_text_present_rerender_allocates_nothing).",
            iters_cap: None,
            make: make_wrapped_rerender_1,
        },
        BenchPath {
            name: "wrapped_frame_clean",
            budget: Budget::Zero,
            note: "clean frame over the retained n=200 WRAPPED doc (all-static paragraphs). The targeted ready queue is empty and the clean-layout check skips emit_wrapped_paint — zero, no matter how much wrapped text is retained (SOUL §3.1, Directive #3).",
            iters_cap: None,
            make: make_wrapped_frame_clean,
        },
        BenchPath {
            name: "text_reflow",
            budget: Budget::Bounded {
                allocs: LONG_TEXT_RESIZE_ALLOCS,
                bytes: LONG_TEXT_RESIZE_BYTES,
            },
            note: "a LONG (~700 glyph) wrapped paragraph whose wrap width cycles through 10 distinct widths between 180 and 260px (more than the measure cache holds, so every iteration shapes a new width; App::resize + frame). Unlike wrap_reflow's 59-char sample this one spills the glyph storage past 16, so it shows what measure+paint shaping costs: the width is shaped once and shared by the measure and paint passes.",
            iters_cap: None,
            make: make_text_reflow,
        },
        BenchPath {
            name: "textarea_type_char",
            budget: Budget::Report,
            note: "insert one character in the middle of a 2000-line TextArea (then Backspace it on the next iteration) + frame. User-initiated edit work: only the edited line is re-hashed, reshaped and re-emitted (the paint is spliced in place); the other 1,999 lines come from the line table. Reported, never gated; the scale_textarea_* rows gate size independence.",
            iters_cap: None,
            make: make_textarea_type_char,
        },
        BenchPath {
            name: "textarea_caret_move",
            budget: Budget::Zero,
            note: "move the caret one line down (then up on the next iteration) in a 2000-line TextArea + frame. Reshapes no line and re-emits no text: only the caret primitive range is rewritten.",
            iters_cap: None,
            make: make_textarea_caret_move,
        },
        BenchPath {
            name: "ellipsis_resize",
            budget: Budget::Bounded {
                allocs: LONG_TEXT_RESIZE_ALLOCS,
                bytes: LONG_TEXT_RESIZE_BYTES,
            },
            note: "a long single-line ellipsis label whose available width cycles through 10 distinct widths between 180 and 260px (App::resize + frame): each width truncates once with a three-shape walk over cluster advances instead of re-shaping prefixes.",
            iters_cap: None,
            make: make_ellipsis_resize,
        },
        BenchPath {
            // Budget cited from the INTEGRATE report / tests/alloc_budget.rs::
            // WRAP_CHANGE_BUDGET — see WRAP_REFLOW_ALLOCS/BYTES above (measured 24 allocs
            // / 15 345 bytes; reviewed ceiling 40 allocs).
            name: "wrap_reflow",
            budget: Budget::Bounded {
                allocs: WRAP_REFLOW_ALLOCS,
                bytes: WRAP_REFLOW_BYTES,
            },
            note: "change the wrap WIDTH (resize-style) of ONE wrapped paragraph + frame: App::resize toggles the viewport 180<->260px, clearing laid_out so the next frame re-measures the wrapped leaf at the new width, re-wraps to a different line count, re-emits multi-line glyphs, AND does a full Taffy relayout. Layout-dirty work — small NON-zero by design (SOUL §4.1 resize+text_edit families). MEASURED 24 allocs / 15 345 bytes in the INTEGRATE report; ceiling pinned at 40 allocs (headroom) mirroring alloc_budget.rs::WRAP_CHANGE_BUDGET.",
            iters_cap: None,
            make: make_wrap_reflow,
        },
    ]
}

/// A proportionality path: the same steady-state closure, parameterised by document
/// size n. The `large_text_*` point rows above are the n=200 samples of these curves.
pub(crate) struct ScalePath {
    pub(crate) name: &'static str,
    pub(crate) note: &'static str,
    pub(crate) make: fn(usize) -> Box<dyn FnMut()>,
    /// The work legitimately grows with n (every one of n nodes runs), so the time
    /// ratio is taken per unit of n: flat means constant cost per node.
    pub(crate) per_n: bool,
}

/// The proportionality suite (SOUL Directive #3 made executable).
pub(crate) fn scale_paths() -> Vec<ScalePath> {
    vec![
        ScalePath {
            name: "scale_rerender_1",
            note: "signal->color paint binding set + frame over an n-paragraph retained doc",
            make: large_rerender_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_text_edit",
            note: "digit swap in the one dynamic text mid an n-paragraph doc + frame",
            make: large_text_edit_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_width_change",
            note: "the one Text::dynamic leaf of an n-row doc (grouped into fixed-size panels, 5 deep and <= 4 wide at every n, so Taffy's own per-container flex pass and node cache stay out of the measurement) changes measured width every iteration (layout-dirty frame); geometry write-back, paint re-anchoring and measure-hook dispatch must follow what moved, not the n rows that did not",
            make: width_change_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_textarea_caret_move",
            note: "caret one line down/up in an n-line TextArea + frame; the caret moves through the cached line table and rewrites only the caret primitives, so the cost may not depend on n",
            make: textarea_caret_move_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_textarea_overflow_caret_move",
            note: "caret one line down/up in a word-wrapped n-line TextArea whose first line is an unbreakable token spilling out of the box; the steady spill must not re-fit the focus ring or rescan the primitives per move",
            make: textarea_overflow_caret_move_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_textarea_type_char",
            note: "type/erase one character mid an n-line TextArea + frame; only the edited line is reshaped and re-emitted, the rest is a splice of the retained paint",
            make: textarea_type_char_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_textarea_scrolled",
            note: "a 20n-line TextArea in a 200px Scroll is jumped three viewports up/down + frame; it emits only the rows near the viewport, so the cost may not depend on the document length",
            make: textarea_scrolled_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_terminal_patch_row",
            note: "a patched 80-column terminal of n rows; the host rewrites ONE row + frame. Dirty rows are a list, ranges move only when a row changes length, and the a11y value is spliced per row, so the cost may not depend on the grid height",
            make: terminal_patch_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_frame_clean",
            note: "clean frame over an n-paragraph retained doc (empty targeted-ready queue)",
            make: large_frame_clean_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_scroll_direct",
            note: "direct App retained-scroll dispatch + delta retirement over an n-row real document; offset mutation and allocation count must remain flat",
            make: scroll_direct_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_wheel_route",
            note: "indexed point-to-scroll routing + retained mutation over an n-row real document; a fallback to leaf-by-leaf hit testing must fail the flat-scaling gate",
            make: wheel_route_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_pointer_move",
            note: "hover + cursor resolution for one pointer move over an n-row real document; hit testing must prune by subtree bounds instead of visiting every row",
            make: pointer_move_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_hover_move",
            note: "hover moves between two buttons of an n-row document with a focus ring on a third + frame; only the controls involved are restyled, so the cost may not depend on the row count",
            make: hover_move_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_virtual_list_append",
            note: "append one row to a VirtualList of 100n rows (1k/10k/100k) + frame; the key index and prefix index grow in place and the mounted window is unchanged, so the cost may not depend on the source length",
            make: virtual_list_append_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_wrapped_frame_clean",
            note: "clean frame over an n-paragraph retained WRAPPED doc — the empty targeted-ready queue and clean-layout gate must not care how much wrapped text is retained (emit_wrapped_paint stays uncalled)",
            make: wrapped_frame_clean_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_dynamic_sites",
            note: "n Text::dynamic sites on n signals; ONE signal changes + frame. Only one subscription is ready and retracked, so the cost may not depend on how many sites the group holds",
            make: dynamic_sites_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_paint_bindings",
            note: "n reactive paint bindings on n signals; ONE signal changes + frame. Only the ready binding is re-evaluated, so a frame may not call or scan the other n-1 closures",
            make: paint_bindings_closure,
            per_n: false,
        },
        ScalePath {
            name: "scale_broadcast_per_observer",
            note: "one signal with n effect observers changes; all n re-run. Time ratio is per observer (work is n runs by construction): edge maintenance must be O(1) per run, not O(observers)",
            make: broadcast_closure,
            per_n: true,
        },
    ]
}

// ---------------------------------------------------------------------------
// Scenario builders (mirrors of tests/alloc_budget.rs + examples/counter).
// ---------------------------------------------------------------------------
