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
// layout runs only if a measured width moved, and each changed node's rect is folded
// into `scene.damage()` ready for a byte upload. It **does not** submit to the GPU —
// wgpu's submission internals allocate inside a foreign crate we do not control, so
// §4.1 explicitly permits splitting the measurement to the CPU-side path. So these
// numbers cover *decide-what-changed + prepare-the-delta*, never the GPU upload.
//
// ## Targeted dynamic-text delivery (the polling floor, closed)
//
// `schnellui_widgets::run_dynamic_slots` drains a per-app ready queue populated by
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

/// The reviewed `text_edit` budget, cited from
/// `crates/schnellui/tests/alloc_budget.rs::TEXT_EDIT_BUDGET` (3 allocs / 23 bytes):
/// (1) the producer's `i64 -> String` format, (2) the change-suppression
/// `last.clone()` in `run_dynamic_slots`, (3) the a11y `value` String (Role::Status).
/// The same ceiling applies at every document size — text_edit cost must not scale
/// with n (Directive #3).
pub(crate) const TEXT_EDIT_ALLOCS: u64 = 3;
pub(crate) const TEXT_EDIT_BYTES: u64 = 23;

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
            name: "mount",
            budget: Budget::Report,
            note: "App::mount_with_size_scaled of a counter-like UI. A first-mount grow event (SOUL §4) — allowed to allocate; the number is reported, never gated. Iters capped at 25 (mount-class).",
            iters_cap: Some(MOUNT_ITERS_CAP),
            make: make_mount,
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
            // (currently 3 allocs / 23 bytes) — see TEXT_EDIT_ALLOCS/BYTES above.
            name: "text_edit",
            budget: Budget::Bounded {
                allocs: TEXT_EDIT_ALLOCS,
                bytes: TEXT_EDIT_BYTES,
            },
            note: "dynamic text digit swap 18<->81 (same width, warm glyphs) + frame. SOUL §4.1 budgets this small/non-zero (see alloc_budget.rs TEXT_EDIT_BUDGET = 3 allocs / 23 bytes).",
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
            budget: Budget::Bounded {
                allocs: TEXT_EDIT_ALLOCS,
                bytes: TEXT_EDIT_BYTES,
            },
            note: "digit swap 18<->81 in the ONE Text::dynamic mid the n=200 doc (same width, warm glyphs). Same 3/23 budget as the small text_edit — the text path must not cost more in a big document (Directive #3).",
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
}

/// The proportionality suite (SOUL Directive #3 made executable).
pub(crate) fn scale_paths() -> Vec<ScalePath> {
    vec![
        ScalePath {
            name: "scale_rerender_1",
            note: "signal->color paint binding set + frame over an n-paragraph retained doc",
            make: large_rerender_closure,
        },
        ScalePath {
            name: "scale_text_edit",
            note: "digit swap in the one dynamic text mid an n-paragraph doc + frame",
            make: large_text_edit_closure,
        },
        ScalePath {
            name: "scale_frame_clean",
            note: "clean frame over an n-paragraph retained doc (empty targeted-ready queue)",
            make: large_frame_clean_closure,
        },
        ScalePath {
            name: "scale_scroll_direct",
            note: "direct App retained-scroll dispatch + delta retirement over an n-row real document; offset mutation and allocation count must remain flat",
            make: scroll_direct_closure,
        },
        ScalePath {
            name: "scale_wheel_route",
            note: "indexed point-to-scroll routing + retained mutation over an n-row real document; a fallback to leaf-by-leaf hit testing must fail the flat-scaling gate",
            make: wheel_route_closure,
        },
        ScalePath {
            name: "scale_wrapped_frame_clean",
            note: "clean frame over an n-paragraph retained WRAPPED doc — the empty targeted-ready queue and clean-layout gate must not care how much wrapped text is retained (emit_wrapped_paint stays uncalled)",
            make: wrapped_frame_clean_closure,
        },
    ]
}

// ---------------------------------------------------------------------------
// Scenario builders (mirrors of tests/alloc_budget.rs + examples/counter).
// ---------------------------------------------------------------------------
