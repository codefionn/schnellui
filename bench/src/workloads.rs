use super::*;

/// The counter UI, constructed at `start` (mirror of examples/counter's `view!` chain).
pub(crate) fn counter_view(start: i64) -> impl View {
    let count = create_signal(start);
    Column::new()
        .child(Text::new("Counter"))
        .child(
            Text::dynamic_into(move |b| {
                let _ = std::fmt::Write::write_fmt(b, format_args!("{}", count.get()));
            })
            .role(Role::Status),
        )
        .child(Button::new("increment").on_click(move || count.set(count.get() + 1)))
        .gap(8.0)
}

/// A headless app whose `n` leaf nodes each carry a signal-bound reactive paint
/// binding — the pure `rerender_1_signal`/`rerender_n_signals` path (SOUL §4.1). Each
/// `set_signal("s{i}", v)` advances signal `i`; `frame()` evaluates the binding and
/// repaints that node's fill. Copy in, Copy out => zero-alloc steady state.
pub(crate) fn paint_app(width: u32, height: u32, n: usize) -> App {
    // Every App owns an isolated widget runtime, so this raw-scene app starts clean.
    let mut app = App::new(width, height);
    let root = app.scene_mut().insert(WidgetKind::Column, None);
    app.scene_mut().set_root(root);
    app.scene_mut()
        .set_rect(root, Rect::new(0.0, 0.0, width as f32, height as f32));

    let slot_h = height as f32 / n as f32;
    for i in 0..n {
        let node = app.scene_mut().insert(WidgetKind::Button, Some(root));
        let rect = Rect::new(0.0, i as f32 * slot_h, width as f32, slot_h);
        app.scene_mut().set_rect(node, rect);
        // Seed a fill for `set_color` to mutate (mount may allocate, SOUL §4).
        app.scene_mut().replace_primitives(
            node,
            [Primitive::SolidRect {
                rect,
                color: Color::BLACK,
                corner_radius: 0.0,
            }],
        );

        let count = create_signal(0i64);
        app.register_signal(format!("s{i}"), move |v| {
            if let TestValue::Int(x) = v {
                count.set(x);
            }
        });
        // signal -> one node's fill colour (Copy in, Copy out => zero-alloc eval).
        app.bind_paint(node, move || {
            let k = count.get();
            Color::rgb((k & 0xFF) as u8, ((k >> 8) & 0xFF) as u8, 0)
        });
    }
    app
}

// ---- the large document (SOUL Directive #3 test bed) ----

/// Varied, realistic ~50-char sentence stock. Mixed case + digits + punctuation so
/// glyph coverage is honest; still only ~70 DISTINCT (glyph, 16px) atlas entries at
/// any n, because the shaper caches per glyph id + size.
const SENTENCES: [&str; 6] = [
    "The quick brown fox jumps over the lazy dog again",
    "Zwei flinke Boxer jagen die quirlige Eva durch Sylt",
    "GPU tiles re-raster only when a fingerprint changed",
    "signals mark dirty; memos pull lazily; effects settle",
    "A retained scene uploads deltas in bytes, not buffers",
    "Paris, Tokyo & Berlin: 60fps on battery (allegedly!)",
];

/// One ~60-char paragraph, varied by index (digits included => the text_edit digits
/// are already realistic atlas residents).
pub(crate) fn paragraph(i: usize) -> String {
    format!("{:>3}. {}", i % 1000, SENTENCES[i % SENTENCES.len()])
}

/// The large doc, text-dynamic variant: n static ~60-char paragraphs plus **ONE**
/// `Text::dynamic` (signal "count", `Role::Status`) in the middle — the honest
/// large-document `text_edit` scenario.
pub(crate) fn large_doc_text(n: usize) -> App {
    let count = create_signal(0i64);
    let mut col = Column::new().gap(4.0);
    for i in 0..n {
        if i == n / 2 {
            col = col.child(
                Text::dynamic_into(move |b| {
                    let _ = std::fmt::Write::write_fmt(b, format_args!("{}", count.get()));
                })
                .role(Role::Status),
            );
        }
        col = col.child(Text::new(paragraph(i)));
    }
    let mut app = App::mount_with_size(col, 800, 600);
    app.register_signal("count", move |v| {
        if let TestValue::Int(i) = v {
            count.set(i);
        }
    });
    app
}

/// The large doc, paint-binding variant: n static paragraphs plus **ONE** dynamic
/// site that is a signal→fill-colour paint binding on a mid-doc swatch node (signal
/// "color") — the covenant's Copy path (the same shape the soul test gates), used by
/// changed-signal rows because its Copy value can remain literal zero-allocation;
/// a changed `Text::dynamic` site necessarily produces and shapes text.
pub(crate) fn large_doc_paint(n: usize) -> App {
    let mut col = Column::new().gap(4.0);
    for i in 0..n {
        col = col.child(Text::new(paragraph(i)));
    }
    let mut app = App::mount_with_size(col, 800, 600);

    // One raw swatch node (single SolidRect — set_color compares one primitive, O(1)).
    let root = app.scene().root().expect("mounted doc has a root");
    let node = app.scene_mut().insert(WidgetKind::Button, Some(root));
    let rect = Rect::new(0.0, 0.0, 800.0, 20.0);
    app.scene_mut().set_rect(node, rect);
    app.scene_mut().replace_primitives(
        node,
        [Primitive::SolidRect {
            rect,
            color: Color::BLACK,
            corner_radius: 0.0,
        }],
    );

    let color = create_signal(0i64);
    app.register_signal("color", move |v| {
        if let TestValue::Int(x) = v {
            color.set(x);
        }
    });
    app.bind_paint(node, move || {
        let k = color.get();
        Color::rgb((k & 0xFF) as u8, ((k >> 8) & 0xFF) as u8, 0)
    });
    app
}

// ---- the WRAPPED large document (SOUL §8.1 text wrap test bed) ----

/// A **stretch** column: `Align::Stretch` hands each child a definite (100%-of-parent)
/// width — the definite wrap width a `WrapMode::Word` `Text` leaf needs to break lines
/// (INTEGRATE report "Gaps": a bare content-sized column won't force wrap). Mirror of
/// `tests/alloc_budget.rs` + `tests/retained_equivalence.rs`'s `stretch_column` helper.
pub(crate) fn stretch_col() -> Column {
    let mut style = ContainerStyle::new(Container::Column);
    style.align = Align::Stretch;
    Column::new().style(style).gap(4.0)
}

/// The n=paragraph WRAPPED document: a stretch column of n static `WrapMode::Word`
/// `Text` leaves. Mounted narrow ([`WRAPPED_WIDTH`]) so each paragraph actually breaks
/// to multiple lines. All-static (no dynamic slot), the bed for `large_wrapped_mount`,
/// `wrapped_frame_clean`, and `scale_wrapped_frame_clean`.
pub(crate) fn wrapped_doc(n: usize) -> App {
    let mut col = stretch_col();
    for i in 0..n {
        col = col.child(Text::new(paragraph(i)).wrap(WrapMode::Word));
    }
    App::mount_with_size(col, WRAPPED_WIDTH, WRAPPED_HEIGHT)
}

// ---------------------------------------------------------------------------
// Path factories: build + warm, return the steady-state closure to measure.
// ---------------------------------------------------------------------------

pub(crate) fn make_signal_set_flush() -> Box<dyn FnMut()> {
    let s = create_signal(0i64);
    let m = create_memo(move || s.get() + 1);
    let sink = Arc::new(AtomicI64::new(0));
    let sink2 = sink.clone();
    // effect subscribes m (-> s); set(s) queues it, flush re-runs it (SOUL §3.1).
    let _e = create_effect(move || sink2.store(m.get(), Ordering::Relaxed));

    // Warm: first set + flush is allowed to grow pools (§4.1).
    s.set(1);
    Runtime::flush();

    let mut n = 1i64;
    Box::new(move || {
        n += 1;
        s.set(black_box(n));
        Runtime::flush();
        // keep the sink Arc alive so the effect chain is not disposed early.
        black_box(sink.load(Ordering::Relaxed));
    })
}

pub(crate) fn make_memo_diamond() -> Box<dyn FnMut()> {
    let s = create_signal(0i64);
    let a = create_memo(move || s.get() + 1); // source -> memo_a
    let b = create_memo(move || s.get() * 2); // source -> memo_b
    let join = create_memo(move || a.get() + b.get()); // {a,b} -> join (the diamond)

    // Warm: settle the graph once so edge lists are stable (§3.1 sentinel reuse).
    s.set(1);
    black_box(join.get());

    let mut n = 1i64;
    Box::new(move || {
        n += 1;
        s.set(black_box(n)); // push: a Dirty, b Dirty, join Check
        black_box(join.get()); // pull: each node recomputed at most once, in place
    })
}

pub(crate) fn make_mount() -> Box<dyn FnMut()> {
    // Warm the global signal arena and app-owned widget runtime with a throwaway mount so we
    // measure the *second-and-later* build (SOUL §4.1). Note: each mount leaks its
    // view's signals into the process-wide arena (the App does not own their scope) —
    // a benign few-KB drift over the run, and exactly why `mount` is Report, not Zero.
    drop(App::mount_with_size_scaled(counter_view(0), 400, 200, 1.0));
    Box::new(|| {
        let app = App::mount_with_size_scaled(counter_view(0), 400, 200, 1.0);
        black_box(&app);
        drop(app);
    })
}

pub(crate) fn make_large_form_remount_state() -> Box<dyn FnMut()> {
    fn form() -> Column {
        let mut form = Column::new();
        for _ in 0..1_000 {
            form = form.child(TextInput::new("value").label("field"));
        }
        form
    }

    let previous = App::mount_with_size(form(), 800, 600);
    let mut replacement = App::mount_with_size(form(), 800, 600);
    Box::new(move || replacement.inherit_remount_state(black_box(&previous)))
}

pub(crate) fn chat_transcript() -> Column {
    let mut transcript = Column::new();
    for index in 0..200 {
        transcript = transcript.child(Text::new(paragraph(index)));
    }
    transcript
}

pub(crate) fn chat_workbench(transcript_ref: ComponentRef) -> Column {
    let mut shell = Column::new();
    for index in 0..500 {
        shell = shell.child(Text::new(paragraph(index + 200)));
    }
    shell = shell.child(chat_transcript().with_ref(transcript_ref));
    for index in 0..500 {
        shell = shell.child(Text::new(paragraph(index + 700)));
    }
    shell
}

pub(crate) fn make_chat_generation_subtree() -> Box<dyn FnMut()> {
    let transcript_ref = ComponentRef::new();
    let mut app = App::mount_with_size(chat_workbench(transcript_ref), 800, 600);
    app.frame();
    black_box(&app);
    Box::new(move || {
        app.replace_subtree(transcript_ref, chat_transcript())
            .expect("transcript ref remains mounted");
        app.frame();
        black_box(&app);
    })
}

pub(crate) fn make_chat_generation_full_remount() -> Box<dyn FnMut()> {
    let transcript_ref = ComponentRef::new();
    let mut app = App::mount_with_size(chat_workbench(transcript_ref), 800, 600);
    app.frame();
    black_box(&app);
    Box::new(move || {
        let mut next = App::mount_with_size(chat_workbench(transcript_ref), 800, 600);
        next.frame();
        let previous = std::mem::replace(&mut app, next);
        black_box(&previous);
        drop(previous);
        black_box(&app);
    })
}

pub(crate) fn make_rerender_1_signal() -> Box<dyn FnMut()> {
    let mut app = paint_app(400, 200, 1);
    app.frame(); // first mount + frame may allocate (grow caches/pools, §4.1)
    app.set_signal("s0", 1i64);
    app.frame(); // warm the second-invocation capacity

    let mut hi = false;
    Box::new(move || {
        hi = !hi;
        // toggle so every frame is a genuine repaint (colour actually changes).
        app.set_signal("s0", black_box(if hi { 2i64 } else { 1i64 }));
        app.frame();
    })
}

pub(crate) fn make_rerender_n_signals() -> Box<dyn FnMut()> {
    const N: usize = 8;
    let mut app = paint_app(400, 240, N);
    // Precompute the signal names ONCE, outside the measured closure — formatting
    // `"s{i}"` inside the closure would allocate N Strings and (correctly!) fail the
    // zero gate on the *harness*, not the framework (SOUL §4.1 warns of exactly this).
    let names: Vec<String> = (0..N).map(|i| format!("s{i}")).collect();

    app.frame();
    for name in &names {
        app.set_signal(name, 1i64);
    }
    app.frame(); // warm second-invocation capacity for all 8 bindings

    let mut hi = false;
    Box::new(move || {
        hi = !hi;
        // set ALL 8 independent signals, then ONE frame settles them together.
        for (i, name) in names.iter().enumerate() {
            let v = if hi { (2 + i) as i64 } else { (1 + i) as i64 };
            app.set_signal(name, black_box(v));
        }
        app.frame();
    })
}

pub(crate) fn make_text_edit() -> Box<dyn FnMut()> {
    // The real through-App counter whose signal change re-formats an integer, re-shapes
    // the label through the pooled Parley context, and flags a one-node a11y update.
    let count = create_signal(0i64);
    let view = Column::new()
        .child(Text::new("Counter"))
        .child(
            Text::dynamic_into(move |b| {
                let _ = std::fmt::Write::write_fmt(b, format_args!("{}", count.get()));
            })
            .role(Role::Status),
        )
        .child(Button::new("increment").on_click(move || count.set(count.get() + 1)));
    let mut app = App::mount_with_size_scaled(view, 400, 200, 1.0);
    app.register_signal("count", move |v| {
        if let TestValue::Int(i) = v {
            count.set(i);
        }
    });

    // Warm: rasterise '1' and '8', grow every pool, settle the 2nd-invocation state.
    app.frame();
    app.set_signal("count", 18i64); // rasterises '1' and '8'
    app.frame();
    app.set_signal("count", 81i64); // same width, digits cached, pools warm
    app.frame();

    let mut hi = false;
    Box::new(move || {
        hi = !hi;
        // digit swap 18<->81: same width (no relayout), both digits already rasterised
        // (no atlas grow) — the reproducible steady-warm text_edit cost.
        app.set_signal("count", black_box(if hi { 18i64 } else { 81i64 }));
        app.frame();
    })
}

/// A focused `TextArea` of `lines` source lines (~50 chars each) with the caret parked
/// mid-document, warmed so every glyph is an atlas resident and every pool has grown.
fn textarea_app(lines: usize) -> App {
    textarea_app_wrapped(lines, WrapMode::NoWrap)
}

fn textarea_app_wrapped(lines: usize, wrap: WrapMode) -> App {
    // Line 0 is wider than any other, so typing never changes the measured size
    // (a layout-dirty edit would add Taffy's allocations at small n only).
    let source = std::iter::once("=".repeat(160))
        .chain((1..lines).map(|i| format!("{:>4} {}", i % 10_000, SENTENCES[i % SENTENCES.len()])))
        .collect::<Vec<_>>()
        .join("\n");
    let mut app = App::mount_with_size(
        Column::new().child(TextArea::new(source).wrap(wrap)),
        800,
        600,
    );
    app.frame();
    let id = app
        .find_widget(Role::MultilineTextInput, None)
        .expect("mounted text area");
    app.focus(Some(id));
    // The caret starts at the end; walk it up to the middle.
    for _ in 0..lines / 2 {
        app.dispatch_edit_key(EditKey::Up { select: false });
    }
    app.dispatch_edit_key(EditKey::Home { select: false });
    app.dispatch_edit_key(EditKey::Right {
        select: false,
        word: false,
    });
    app.frame();
    app
}

/// Insert one character in the middle of an `lines`-line TextArea, then remove it on
/// the next iteration (so the document stays the same size), + frame.
pub(crate) fn textarea_type_char_closure(lines: usize) -> Box<dyn FnMut()> {
    let mut app = textarea_app(lines);
    // Warm: rasterise 'x', grow pools, settle the second-invocation state.
    for _ in 0..3 {
        app.dispatch_edit_key(EditKey::Insert("x"));
        app.frame();
        app.dispatch_edit_key(EditKey::Backspace);
        app.frame();
    }
    let mut typed = false;
    Box::new(move || {
        typed = !typed;
        if typed {
            app.dispatch_edit_key(black_box(EditKey::Insert("x")));
        } else {
            app.dispatch_edit_key(black_box(EditKey::Backspace));
        }
        app.frame();
    })
}

/// Move the caret one line down (then up on the next iteration) in the middle of an
/// `lines`-line TextArea + frame.
pub(crate) fn textarea_caret_move_closure(lines: usize) -> Box<dyn FnMut()> {
    let mut app = textarea_app(lines);
    for _ in 0..4 {
        app.dispatch_edit_key(EditKey::Down { select: false });
        app.frame();
        app.dispatch_edit_key(EditKey::Up { select: false });
        app.frame();
    }
    let mut down = false;
    Box::new(move || {
        down = !down;
        if down {
            app.dispatch_edit_key(black_box(EditKey::Down { select: false }));
        } else {
            app.dispatch_edit_key(black_box(EditKey::Up { select: false }));
        }
        app.frame();
    })
}

/// Caret one line down/up in a word-wrapped `lines`-line TextArea whose first line is
/// one unbreakable token wider than the box, so it spills out of the surface for good.
/// The focus ring must not be re-fitted per move.
pub(crate) fn textarea_overflow_caret_move_closure(lines: usize) -> Box<dyn FnMut()> {
    let mut app = textarea_app_wrapped(lines, WrapMode::Word);
    for _ in 0..4 {
        app.dispatch_edit_key(EditKey::Down { select: false });
        app.frame();
        app.dispatch_edit_key(EditKey::Up { select: false });
        app.frame();
    }
    let mut down = false;
    Box::new(move || {
        down = !down;
        if down {
            app.dispatch_edit_key(black_box(EditKey::Down { select: false }));
        } else {
            app.dispatch_edit_key(black_box(EditKey::Up { select: false }));
        }
        app.frame();
    })
}

/// A TextArea of `20 * n` lines in a 200px Scroll, jumped a few viewports up and
/// down each iteration + frame. The area keeps only the rows near the viewport
/// (plus overscan) as primitives, so a jump re-emits one band whatever the
/// document length.
pub(crate) fn textarea_scrolled_closure(n: usize) -> Box<dyn FnMut()> {
    let lines = n * 20;
    let source = (0..lines)
        .map(|i| format!("{:>5} {}", i % 100_000, SENTENCES[i % SENTENCES.len()]))
        .collect::<Vec<_>>()
        .join("\n");
    let mut app = App::mount_with_size(
        Column::new().child(
            Scroll::new()
                .label("bench text area scroll")
                .size(600.0, 200.0)
                .child(Flex::new().shrink(0.0).child(TextArea::new(source))),
        ),
        800,
        600,
    );
    app.frame();
    let scroll = app
        .find_widget(Role::ScrollView, Some("bench text area scroll"))
        .expect("scroll viewport mounted");
    let at = move |app: &mut App, y: f32| {
        app.scene_mut()
            .set_scroll_offset(scroll, Point { x: 0.0, y });
        app.frame();
    };
    for _ in 0..3 {
        at(&mut app, 900.0);
        at(&mut app, 0.0);
    }
    let mut down = false;
    Box::new(move || {
        down = !down;
        at(&mut app, black_box(if down { 900.0 } else { 0.0 }));
    })
}

pub(crate) fn make_textarea_type_char() -> Box<dyn FnMut()> {
    textarea_type_char_closure(TEXTAREA_LINES)
}

pub(crate) fn make_textarea_caret_move() -> Box<dyn FnMut()> {
    textarea_caret_move_closure(TEXTAREA_LINES)
}

pub(crate) fn make_frame_clean() -> Box<dyn FnMut()> {
    let mut app = App::new(320, 240);
    let node = app.scene_mut().insert(WidgetKind::Button, None);
    app.scene_mut().set_root(node);
    let rect = Rect::new(0.0, 0.0, 320.0, 240.0);
    app.scene_mut().set_rect(node, rect);
    app.scene_mut().replace_primitives(
        node,
        [Primitive::SolidRect {
            rect,
            color: Color::BLACK,
            corner_radius: 0.0,
        }],
    );
    app.frame(); // lays out once
    app.frame(); // now nothing is dirty

    Box::new(move || {
        // No signal set or binding write: both reactive ready queues are empty.
        app.frame();
    })
}

// ---- retained scrolling (SOUL §3.2 property mutation, §4.1 zero budget) ----

/// A realistic retained scroll viewport: one thousand plain text rows live below a
/// clipped 600px viewport. The content is intentionally ordinary (not a synthetic
/// raw scene) so dispatch measures the same widgets/app boundary hosts use.
fn scroll_doc(n: usize) -> App {
    // The column's content extent is explicit: without an authored height, flex
    // shrink can collapse a root scroll's only child to its viewport height before
    // the benchmark reaches the retained scroll path.
    let mut rows = Column::new().gap(2.0).height((n as f32 * 20.0).max(480.0));
    for i in 0..n {
        rows = rows.child(Text::new(format!("Row {i}")));
    }
    App::mount_with_size(
        // Keep the viewport below a normal retained root: this mirrors an app
        // panel rather than relying on root-scroll sizing special cases.
        Column::new().child(
            Scroll::new()
                .label("bench long scroll")
                .size(320.0, 220.0)
                // A scroll child must opt out of flex shrinking or Taffy clamps
                // it to the viewport before `scroll_metrics` sees its extent.
                .child(Flex::new().shrink(0.0).child(rows)),
        ),
        400,
        300,
    )
}

/// Assert and retire the exact retained mutation a renderer would consume. This
/// keeps the benchmark on the CPU event→property-delta path while proving scroll
/// neither requests nor performs layout (SOUL §3.2).
fn assert_and_retire_scroll_delta(app: &mut App, scroll: schnellui::scene::WidgetId, before: f32) {
    let after = app.scene().scroll_offset(scroll).y;
    assert_ne!(after, before, "bench delta must move the retained offset");
    assert!(
        app.scene().layout_dirty().is_empty(),
        "scroll must mutate only composition/a11y state, never layout"
    );
    assert!(
        !app.scene().paint_dirty().is_empty(),
        "scroll must produce a retained paint/composition delta"
    );
    app.scene_mut().clear_dirty();
}

/// Direct App→retained-scroll dispatch at `n` rows. The public App seam avoids
/// measuring a synthetic duplicate of the widget runtime; it deliberately does not
/// render, so GPU gather/upload remains outside this CPU allocation benchmark.
pub(crate) fn scroll_direct_closure(n: usize) -> Box<dyn FnMut()> {
    let mut app = scroll_doc(n);
    app.frame();
    app.scene_mut().clear_dirty();
    let scroll = app
        .find_widget(Role::ScrollView, Some("bench long scroll"))
        .expect("long scroll viewport mounted");

    assert!(
        app.dispatch_scroll(scroll, 48.0),
        "long scroll must be movable; max offset is {}",
        schnellui::widgets::scroll_metrics(app.scene(), scroll)
            .expect("scroll metrics after layout")
            .max_offset
    );
    assert_and_retire_scroll_delta(&mut app, scroll, 0.0);

    let mut down = false;
    Box::new(move || {
        let before = app.scene().scroll_offset(scroll).y;
        down = !down;
        let delta = if down { 48.0 } else { -48.0 };
        assert!(app.dispatch_scroll(scroll, black_box(delta)));
        assert_and_retire_scroll_delta(&mut app, scroll, before);
    })
}

pub(crate) fn make_scroll_direct_long() -> Box<dyn FnMut()> {
    scroll_direct_closure(1_000)
}

/// Full mouse-wheel routing at a long document: hit-test an actual pointer point,
/// resolve its innermost viewport, then take the same retained mutation as direct
/// dispatch. This is report-only because the current tree walk is intentionally
/// exposed rather than hidden behind a flaky absolute-time threshold.
pub(crate) fn wheel_route_closure(n: usize) -> Box<dyn FnMut()> {
    let mut app = scroll_doc(n);
    app.frame();
    app.scene_mut().clear_dirty();
    let scroll = app
        .find_widget(Role::ScrollView, Some("bench long scroll"))
        .expect("long scroll viewport mounted");
    let point = Point { x: 24.0, y: 24.0 };

    assert!(app.dispatch_wheel_at(point, 48.0));
    assert_and_retire_scroll_delta(&mut app, scroll, 0.0);

    let mut down = false;
    Box::new(move || {
        let before = app.scene().scroll_offset(scroll).y;
        down = !down;
        let delta = if down { 48.0 } else { -48.0 };
        assert!(app.dispatch_wheel_at(point, black_box(delta)));
        assert_and_retire_scroll_delta(&mut app, scroll, before);
    })
}

/// A pointer move over an n-row document: hover resolution plus semantic cursor
/// resolution, the pair a windowed `CursorMoved` runs. Two points alternate so
/// neither is served from a one-entry memo. Hover and cursor must both resolve
/// without walking the rows.
pub(crate) fn pointer_move_closure(n: usize) -> Box<dyn FnMut()> {
    let mut app = scroll_doc(n);
    app.frame();
    // An overlay that existed once (a context menu opened and dismissed) leaves a
    // high slot behind in the overlay column; hit testing must not pay for it.
    {
        let scene = app.scene_mut();
        let root = scene.root();
        let popup = scene.insert(WidgetKind::Column, root);
        scene.set_overlay(popup);
        scene.remove_subtree(popup);
    }
    app.scene_mut().clear_dirty();
    let points = [Point { x: 24.0, y: 24.0 }, Point { x: 24.0, y: 60.0 }];
    let mut next = 0;
    let mut step = move |app: &mut App| {
        next ^= 1;
        let point = points[next];
        let _ = black_box(app.update_pointer_proximity(black_box(point)));
        black_box(app.cursor_at(black_box(point)));
    };
    // Warm any lazily built index before measurement starts.
    step(&mut app);
    step(&mut app);
    Box::new(move || step(&mut app))
}

/// Pointer moves between two buttons of an n-row document while a third wears the
/// keyboard focus ring, plus a frame: a windowed `CursorMoved` that crosses a
/// control boundary. The hover decoration changes hands, and only the controls
/// involved may be restyled, so the cost cannot follow the row count.
pub(crate) fn hover_move_closure(n: usize) -> Box<dyn FnMut()> {
    let mut rows = Column::new().gap(2.0);
    for i in 0..n {
        rows = rows.child(Text::new(paragraph(i)));
    }
    let mut app = App::mount_with_size(
        Column::new()
            .child(Button::new("hover alpha"))
            .child(Button::new("hover beta"))
            .child(Button::new("focus gamma"))
            .child(rows),
        800,
        600,
    );
    app.frame();
    let find = |app: &App, name: &str| {
        app.find_widget(Role::Button, Some(name))
            .expect("bench button mounted")
    };
    let (alpha, beta, gamma) = (
        find(&app, "hover alpha"),
        find(&app, "hover beta"),
        find(&app, "focus gamma"),
    );
    assert!(app.focus(Some(gamma)), "the third button takes focus");
    let center = |app: &App, id| {
        let rect = app.scene().layout(id).expect("laid out").rect;
        Point {
            x: rect.x + rect.width * 0.5,
            y: rect.y + rect.height * 0.5,
        }
    };
    let points = [center(&app, alpha), center(&app, beta)];
    app.frame();
    app.scene_mut().clear_dirty();
    let mut next = 0;
    let mut step = move |app: &mut App| {
        next ^= 1;
        assert!(app.update_pointer_proximity(black_box(points[next])));
        app.frame();
        app.scene_mut().clear_dirty();
    };
    step(&mut app);
    step(&mut app);
    Box::new(move || step(&mut app))
}

pub(crate) fn make_hover_move_between_buttons() -> Box<dyn FnMut()> {
    hover_move_closure(1_000)
}

/// Rows per unit of n in the virtual-list series, so the three scale points are
/// 1k, 10k and 100k source rows.
const VIRTUAL_ROWS_PER_N: usize = 100;

/// Appends one row to the end of a long virtual list and runs the frame that
/// reconciles it, as a streaming transcript does. Only the pixel window is ever
/// mounted, so neither the append nor the frame may pay for the source length.
pub(crate) fn virtual_list_append_closure(n: usize) -> Box<dyn FnMut()> {
    let rows = n * VIRTUAL_ROWS_PER_N;
    let controller = VirtualListController::new(0..rows as u64, 20.0);
    let mut app = App::mount_with_size(
        Column::new().child(
            VirtualList::new(controller.clone(), |key: &u64| {
                Text::new(format!("Row {key}"))
            })
            .label("bench virtual list")
            .size(320.0, 220.0),
        ),
        400,
        300,
    );
    app.frame();
    app.frame();
    app.scene_mut().clear_dirty();
    let mut next = rows as u64;
    let mut step = move |app: &mut App| {
        controller.insert(controller.len(), black_box(next));
        next += 1;
        app.frame();
        app.scene_mut().clear_dirty();
    };
    step(&mut app);
    step(&mut app);
    Box::new(move || step(&mut app))
}

pub(crate) fn make_virtual_list_append() -> Box<dyn FnMut()> {
    virtual_list_append_closure(100)
}

pub(crate) fn make_wheel_route_long() -> Box<dyn FnMut()> {
    wheel_route_closure(1_000)
}

/// Schedules and fires a real trailing scroll callback on the App host boundary.
/// `fire_due_scroll_callbacks_at` takes the retained callback out while it runs and
/// restores it afterwards; this covers the former per-fire dummy-`Box` allocation.
pub(crate) fn make_scroll_debounce_due() -> Box<dyn FnMut()> {
    let delivered = Arc::new(AtomicI64::new(0));
    let sink = delivered.clone();
    let rows = Column::new().child(Text::new(paragraph(0)));
    let mut app = App::mount_with_size(
        Scroll::new()
            .label("bench debounce scroll")
            .size(800.0, 100.0)
            .on_scroll_debounced(
                std::time::Duration::from_millis(8),
                std::time::Duration::from_millis(32),
                move |offset| sink.store(offset as i64, Ordering::Relaxed),
            )
            .child(Column::new().child(rows).child(scroll_rows(32))),
        800,
        600,
    );
    app.frame();
    app.scene_mut().clear_dirty();
    let scroll = app
        .find_widget(Role::ScrollView, Some("bench debounce scroll"))
        .expect("debounced scroll viewport mounted");

    // Warm a complete schedule→due cycle so the measured pass starts after every
    // map/string/callback capacity grow event.
    assert!(app.dispatch_scroll(scroll, 48.0));
    let due = app
        .next_scroll_callback_deadline()
        .expect("callback scheduled");
    assert!(app.fire_due_scroll_callbacks_at(due));
    app.scene_mut().clear_dirty();

    let mut down = false;
    Box::new(move || {
        down = !down;
        let delta = if down { 48.0 } else { -48.0 };
        assert!(app.dispatch_scroll(scroll, black_box(delta)));
        let due = app
            .next_scroll_callback_deadline()
            .expect("callback scheduled");
        assert!(app.fire_due_scroll_callbacks_at(due));
        assert_ne!(delivered.load(Ordering::Relaxed), 0);
        assert!(app.scene().layout_dirty().is_empty());
        app.scene_mut().clear_dirty();
    })
}

fn scroll_rows(n: usize) -> Column {
    let mut rows = Column::new().gap(2.0);
    for i in 0..n {
        rows = rows.child(Text::new(paragraph(i)));
    }
    rows
}

// ---- signal graph bookkeeping (signal-graph-perf) ----

/// n reactive paint bindings, each on its own signal; one signal changes per
/// iteration. Only that binding is ready, so the frame must cost the same at every n.
pub(crate) fn paint_bindings_closure(n: usize) -> Box<dyn FnMut()> {
    let mut app = paint_app(800, 600, n);
    app.frame();
    let target = format!("s{}", n / 2);
    app.set_signal(&target, 1i64);
    app.frame();
    app.set_signal(&target, 2i64);
    app.frame();
    let mut hi = false;
    Box::new(move || {
        hi = !hi;
        app.set_signal(&target, black_box(if hi { 1i64 } else { 2i64 }));
        app.frame();
    })
}

/// n `Text::dynamic` sites, each bound to its own signal; one mid-doc signal changes
/// per iteration. Only one subscription becomes ready, so the frame must cost the same
/// at every n (the retrack of a ready subscription may not scan its group).
pub(crate) fn dynamic_sites_closure(n: usize) -> Box<dyn FnMut()> {
    let target = create_signal(0i64);
    let mut col = Column::new().gap(4.0);
    for i in 0..n {
        if i == n / 2 {
            col = col.child(Text::dynamic_into(move |b| {
                let _ = std::fmt::Write::write_fmt(b, format_args!("{}", target.get()));
            }));
        } else {
            let other = create_signal(i as i64);
            col = col.child(Text::dynamic_into(move |b| {
                let _ = std::fmt::Write::write_fmt(b, format_args!("{}", other.get()));
            }));
        }
    }
    let mut app = App::mount_with_size(col, 800, 600);
    app.register_signal("target", move |v| {
        if let TestValue::Int(i) = v {
            target.set(i);
        }
    });
    app.frame();
    app.set_signal("target", 18i64);
    app.frame();
    app.set_signal("target", 81i64);
    app.frame();

    let mut hi = false;
    Box::new(move || {
        hi = !hi;
        app.set_signal("target", black_box(if hi { 18i64 } else { 81i64 }));
        app.frame();
    })
}

/// One signal read by `n` effects; each iteration changes it and re-runs all of them.
/// Every re-run rebuilds the same single edge, so the per-observer cost must not grow
/// with the signal's observer count.
pub(crate) fn broadcast_closure(n: usize) -> Box<dyn FnMut()> {
    let s = create_signal(0i64);
    let sink = Arc::new(AtomicI64::new(0));
    let effects: Vec<_> = (0..n)
        .map(|_| {
            let sink = sink.clone();
            create_effect(move || {
                sink.fetch_add(s.get(), Ordering::Relaxed);
            })
        })
        .collect();
    s.set(1);
    Runtime::flush();
    s.set(2);
    Runtime::flush();

    let mut v = 2i64;
    Box::new(move || {
        v += 1;
        s.set(black_box(v));
        Runtime::flush();
        black_box((&effects, sink.load(Ordering::Relaxed)));
    })
}

pub(crate) fn make_signal_broadcast_1000() -> Box<dyn FnMut()> {
    broadcast_closure(1000)
}

// ---- large-document factories (point rows are the n=200 samples of the curves) ----

/// One theme-transition frame over a 200-paragraph themed app: flips the platform
/// scheme with a zero transition duration, which runs the same `rebuild_with_theme`
/// that every animation frame of a timed transition runs (fresh `App`, full mount,
/// state transfer). The text engine is carried over, so the measured cost excludes
/// re-rasterizing the ~70 distinct glyphs and allocating a new atlas.
pub(crate) fn make_theme_rebuild_frame() -> Box<dyn FnMut()> {
    let mode = schnellui::ThemeMode::system(schnellui::theme::LIGHT, schnellui::theme::DARK);
    let mut app = App::mount_themed(mode, || {
        let mut col = Column::new().gap(4.0);
        for i in 0..200 {
            col = col.child(Text::new(paragraph(i)));
        }
        col
    });
    app.frame();
    let mut dark = false;
    // Warm: the first flip rasterizes nothing new, but grows pools (grow event, §4).
    for _ in 0..2 {
        dark = !dark;
        app.apply_color_scheme(if dark {
            schnellui::platform::ColorScheme::Dark
        } else {
            schnellui::platform::ColorScheme::Light
        });
    }
    Box::new(move || {
        dark = !dark;
        let changed = app.apply_color_scheme(black_box(if dark {
            schnellui::platform::ColorScheme::Dark
        } else {
            schnellui::platform::ColorScheme::Light
        }));
        black_box(changed);
    })
}

pub(crate) fn make_large_text_mount() -> Box<dyn FnMut()> {
    // Warm global pools with a throwaway build; measure the second-and-later mount.
    drop(large_doc_text(200));
    Box::new(|| {
        let app = large_doc_text(200);
        black_box(&app);
        drop(app);
    })
}

/// n-parameterised `rerender_1` over the large doc (paint-binding dynamic site).
pub(crate) fn large_rerender_closure(n: usize) -> Box<dyn FnMut()> {
    let mut app = large_doc_paint(n);
    app.frame(); // first frame: layout the n paragraphs, warm pools (grow event, §4)
    app.set_signal("color", 1i64);
    app.frame(); // warm the second-invocation capacity

    let mut hi = false;
    Box::new(move || {
        hi = !hi;
        app.set_signal("color", black_box(if hi { 2i64 } else { 1i64 }));
        app.frame();
    })
}

/// n-parameterised `text_edit` over the large doc (the one Text::dynamic mid-doc).
pub(crate) fn large_text_edit_closure(n: usize) -> Box<dyn FnMut()> {
    let mut app = large_doc_text(n);
    app.frame();
    app.set_signal("count", 18i64); // rasterise '1'/'8' (already atlas residents)
    app.frame();
    app.set_signal("count", 81i64); // same width, digits cached, pools warm
    app.frame();

    let mut hi = false;
    Box::new(move || {
        hi = !hi;
        app.set_signal("count", black_box(if hi { 18i64 } else { 81i64 }));
        app.frame();
    })
}

/// The large doc, grouped: n paragraphs, the middle one a `Text::dynamic` (signal
/// "count", `Role::Status`) as in [`large_doc_text`], nested [`GROUP_DEPTH`] panels
/// deep with at most 4 children per panel at every n, each panel of definite size.
///
/// The shape is what makes the row measure this engine. Taffy's flex pass is linear
/// in a container's child count (and allocates per recomputed container), and its
/// per-node cache only holds when a container is queried the same way every frame
/// (a content-sized group inside a content-sized group is queried with several
/// available widths that evict each other, so one dirty leaf re-measures the whole
/// document). Neither is this engine's to change, and either would bury geometry
/// write-back, paint re-anchoring and measure-hook dispatch. With a fixed depth and
/// fan-out the dirty path costs the same at every n, so any remaining growth is ours.
pub(crate) fn grouped_doc_text(n: usize) -> App {
    const GROUP_DEPTH: usize = 5;
    const FAN_OUT: usize = 4;
    // Content height (one text line is about 20px) of a panel `depth` levels above
    // the leaf rows.
    fn panel_height(depth: usize) -> f32 {
        let mut height = FAN_OUT as f32 * 24.0 + 100.0;
        for _ in 0..depth {
            height = FAN_OUT as f32 * (height + 4.0) + 100.0;
        }
        height
    }
    fn panel(depth: usize) -> Column {
        let mut style = ContainerStyle::new(Container::Column);
        style.fixed_size = Some(schnellui::scene::Size {
            width: 700.0,
            height: panel_height(depth),
        });
        Column::new().style(style).gap(4.0)
    }
    fn build(
        items: std::ops::Range<usize>,
        depth: usize,
        n: usize,
        count: schnellui::signal::Signal<i64>,
    ) -> Column {
        let mut column = panel(depth);
        if depth == 0 {
            for i in items {
                column = if i == n / 2 {
                    column.child(
                        Text::dynamic_into(move |b| {
                            let _ = std::fmt::Write::write_fmt(b, format_args!("{}", count.get()));
                        })
                        .role(Role::Status),
                    )
                } else {
                    column.child(Text::new(paragraph(i)))
                };
            }
            return column;
        }
        let chunk = items.len().div_ceil(FAN_OUT).max(1);
        let mut start = items.start;
        while start < items.end {
            let end = (start + chunk).min(items.end);
            column = column.child(build(start..end, depth - 1, n, count));
            start = end;
        }
        column
    }
    let count = create_signal(0i64);
    let root = build(0..n, GROUP_DEPTH, n, count);
    let mut app = App::mount_with_size(root, 800, 600);
    app.register_signal("count", move |v| {
        if let TestValue::Int(i) = v {
            count.set(i);
        }
    });
    app
}

/// n-parameterised width change: the one `Text::dynamic` leaf of the grouped doc
/// swaps between a 1-digit and a 6-digit value, so its measured width changes every
/// iteration and the frame takes the layout path (Taffy relayout + geometry
/// write-back + paint re-anchoring). Layout must cost what moved, not the n rows
/// that stay where they are.
pub(crate) fn width_change_closure(n: usize) -> Box<dyn FnMut()> {
    let mut app = grouped_doc_text(n);
    app.frame();
    app.set_signal("count", 1i64); // rasterise both widths' glyphs before measuring
    app.frame();
    app.set_signal("count", 100_000i64);
    app.frame();
    app.set_signal("count", 1i64);
    app.frame();

    let mut wide = false;
    Box::new(move || {
        wide = !wide;
        app.set_signal("count", black_box(if wide { 100_000i64 } else { 1i64 }));
        app.frame();
    })
}

/// n-parameterised clean frame over the large doc (paint-binding variant, with no
/// ready retained subscription and no producer evaluation).
pub(crate) fn large_frame_clean_closure(n: usize) -> Box<dyn FnMut()> {
    let mut app = large_doc_paint(n);
    app.frame();
    app.frame(); // settled: nothing dirty from here on

    Box::new(move || {
        app.frame();
    })
}

pub(crate) fn make_large_text_rerender_1() -> Box<dyn FnMut()> {
    large_rerender_closure(200)
}

pub(crate) fn make_large_text_edit() -> Box<dyn FnMut()> {
    large_text_edit_closure(200)
}

pub(crate) fn make_large_text_frame_clean() -> Box<dyn FnMut()> {
    large_frame_clean_closure(200)
}

/// The polling-floor finding: same doc as `large_text_edit`, but the signal is NEVER
/// set inside the closure — every allocation measured here is pure per-frame polling
/// overhead of one Text::dynamic site (producer String + registry last.clone()).
pub(crate) fn make_dyn_text_poll_clean() -> Box<dyn FnMut()> {
    let mut app = large_doc_text(200);
    app.frame();
    app.set_signal("count", 18i64);
    app.frame();
    app.frame(); // settled; the slot still polls every frame

    Box::new(move || {
        app.frame(); // no set: any alloc here is clean-frame scheduling overhead
    })
}

// ---- wrapped-document factories (SOUL §8.1) ----

pub(crate) fn make_large_wrapped_mount() -> Box<dyn FnMut()> {
    // Warm global pools with a throwaway wrapped build; measure the 2nd-and-later mount.
    drop(wrapped_doc(WRAPPED_N));
    Box::new(|| {
        let app = wrapped_doc(WRAPPED_N);
        black_box(&app);
        drop(app);
    })
}

/// n=200 wrapped paragraphs with ONE signal->color paint binding on a Button mid-doc.
/// A paint-only change keeps layout clean (emit_wrapped_paint uncalled) and the static
/// wrapped leaves never enter `slots`, so the version-gate early-returns => literal zero
/// with the wrapped text merely retained (mirror of the covenant's `rerender_1_signal`
/// shape, guarded by alloc_budget.rs::wrapped_text_present_rerender_allocates_nothing).
pub(crate) fn make_wrapped_rerender_1() -> Box<dyn FnMut()> {
    let count = create_signal(0i64);
    let mut col = stretch_col();
    for i in 0..WRAPPED_N {
        // The paint target sits mid-document, surrounded by retained wrapped text.
        if i == WRAPPED_N / 2 {
            col = col.child(Button::new("swatch"));
        }
        col = col.child(Text::new(paragraph(i)).wrap(WrapMode::Word));
    }
    let mut app = App::mount_with_size(col, WRAPPED_WIDTH, WRAPPED_HEIGHT);
    let node = app
        .find_widget(Role::Button, Some("swatch"))
        .expect("mid-doc swatch button present");
    // signal -> one node's fill colour (Copy in, Copy out => zero-alloc eval).
    app.bind_paint(node, move || {
        let k = count.get();
        Color::rgb((k & 0xFF) as u8, ((k >> 8) & 0xFF) as u8, 0)
    });
    app.register_signal("color", move |v| {
        if let TestValue::Int(x) = v {
            count.set(x);
        }
    });

    app.frame(); // first mount + frame lays out + emits wrapped glyphs (grow, §4)
    app.set_signal("color", 1i64);
    app.frame(); // warm the second-invocation capacity

    let mut hi = false;
    Box::new(move || {
        hi = !hi;
        app.set_signal("color", black_box(if hi { 2i64 } else { 1i64 }));
        app.frame();
    })
}

pub(crate) fn make_wrapped_frame_clean() -> Box<dyn FnMut()> {
    wrapped_frame_clean_closure(WRAPPED_N)
}

/// n-parameterised clean frame over the retained WRAPPED doc (the point row is the
/// n=200 sample of `scale_wrapped_frame_clean`).
pub(crate) fn wrapped_frame_clean_closure(n: usize) -> Box<dyn FnMut()> {
    let mut app = wrapped_doc(n);
    app.frame(); // lays out + emits wrapped glyphs once (grow event, §4)
    app.frame(); // settled: nothing dirty, emit_wrapped_paint no longer runs

    Box::new(move || {
        app.frame();
    })
}

/// Change the wrap WIDTH (resize-style) of ONE wrapped paragraph + frame. `App::resize`
/// toggles the viewport between [`REFLOW_NARROW`] and [`REFLOW_WIDE`], clearing
/// `laid_out` so the next frame re-measures the wrapped leaf at the new width, re-wraps
/// (to a different line count), re-emits its multi-line glyphs, and does a full Taffy
/// relayout — the layout-dirty, small-nonzero `wrap_reflow` budget path (SOUL §4.1).
pub(crate) fn make_wrap_reflow() -> Box<dyn FnMut()> {
    let view = stretch_col().child(Text::new(REFLOW_PARAGRAPH).wrap(WrapMode::Word));
    let mut app = App::mount_with_size(view, REFLOW_NARROW as u32, 400);

    // Warm: wrap at BOTH widths so glyphs are rasterised and every pool is grown, then
    // settle back so the measured pass is the second-and-later re-wrap (SOUL §4.1).
    app.frame(); // wrap at narrow
    app.resize(REFLOW_WIDE, 400.0);
    app.frame(); // re-wrap at wide (grow)
    app.resize(REFLOW_NARROW, 400.0);
    app.frame(); // re-wrap at narrow (pools warm)

    let mut wide = false;
    Box::new(move || {
        wide = !wide;
        let w = if wide { REFLOW_WIDE } else { REFLOW_NARROW };
        app.resize(black_box(w), 400.0);
        app.frame();
    })
}

/// Distinct widths [`resize_cycle`] steps through; more than the 8-entry measure
/// cache, so every iteration misses it and shapes the new width.
const RESIZE_CYCLE_WIDTHS: usize = 10;

/// Warms `app` over every cycle width, then returns a closure that resizes it to
/// the next width of the cycle and runs a frame.
fn resize_cycle(mut app: App) -> Box<dyn FnMut()> {
    let width = |step: usize| {
        let t = step as f32 / (RESIZE_CYCLE_WIDTHS - 1) as f32;
        REFLOW_NARROW + (REFLOW_WIDE - REFLOW_NARROW) * t
    };
    for step in 0..RESIZE_CYCLE_WIDTHS {
        app.resize(width(step), 400.0);
        app.frame();
    }
    let mut step = 0;
    Box::new(move || {
        step = (step + 1) % RESIZE_CYCLE_WIDTHS;
        app.resize(black_box(width(step)), 400.0);
        app.frame();
    })
}

/// A paragraph of roughly 700 glyphs, long enough that its shaped glyphs do not fit
/// the inline storage of a `ShapedText`.
fn long_paragraph() -> String {
    REFLOW_PARAGRAPH.repeat(12)
}

/// `wrap_reflow` with a long paragraph: the shaping, not the Taffy relayout, is what
/// this row is about.
pub(crate) fn make_text_reflow() -> Box<dyn FnMut()> {
    let view = stretch_col().child(Text::new(long_paragraph()).wrap(WrapMode::Word));
    let mut app = App::mount_with_size(view, REFLOW_NARROW as u32, 400);
    app.frame();
    app.resize(REFLOW_WIDE, 400.0);
    app.frame();
    app.resize(REFLOW_NARROW, 400.0);
    app.frame();

    resize_cycle(app)
}

/// An ellipsis label whose available width changes every iteration, so each frame
/// truncates the same long text to a new width.
pub(crate) fn make_ellipsis_resize() -> Box<dyn FnMut()> {
    let view = stretch_col().child(Text::new(long_paragraph()).ellipsis());
    let mut app = App::mount_with_size(view, REFLOW_NARROW as u32, 400);
    app.frame();
    app.resize(REFLOW_WIDE, 400.0);
    app.frame();
    app.resize(REFLOW_NARROW, 400.0);
    app.frame();

    resize_cycle(app)
}

// ---- host-patched terminal and streamed browser frames ----

const TERMINAL_ROW_A: &str = "user@host:~/src/schnellui$ cargo test -p schnellui --features count";
const TERMINAL_ROW_B: &str = "tnuoc serutaef-- iullenhcs p- tset ograc $iullenhcs/crs/~:tsoh@resu";

/// A `rows` x 80 `TerminalGrid::patched` whose host rewrites row 3 on every
/// revision, alternating two strings over the same glyphs. Returns the closure
/// that bumps the revision and runs the frame (the PTY-output path: no signal,
/// the frame polls the revision).
pub(crate) fn terminal_patch_closure(rows: usize) -> Box<dyn FnMut()> {
    use schnellui::widgets::{TerminalCellMetrics, TerminalGrid, TerminalGridPatch};
    use std::cell::Cell;
    use std::rc::Rc;
    let revision = Rc::new(Cell::new(0_u64));
    let source = revision.clone();
    let host = revision.clone();
    let grid = TerminalGrid::patched(
        move || source.get(),
        move |patch: &mut TerminalGridPatch<'_>| {
            patch.resize(80, rows);
            let text = if host.get().is_multiple_of(2) {
                TERMINAL_ROW_A
            } else {
                TERMINAL_ROW_B
            };
            for (cell, (at, ch)) in patch.row_mut(3).iter_mut().zip(text.char_indices()) {
                cell.grapheme = text[at..at + ch.len_utf8()].into();
            }
        },
    )
    .metrics(TerminalCellMetrics::new(8.0, 16.0, 13.0));
    let mut app = App::mount_with_size(grid, 640, (rows * 16).min(4096) as u32);
    for _ in 0..4 {
        revision.set(revision.get() + 1);
        app.frame();
    }
    Box::new(move || {
        revision.set(black_box(revision.get() + 1));
        app.frame();
    })
}

pub(crate) fn make_terminal_patch_row() -> Box<dyn FnMut()> {
    terminal_patch_closure(24)
}

/// A 640x360 RGBA browser frame streamed through `Image::dynamic_rgba_versioned`:
/// every iteration publishes the other of two same-size frames and runs the
/// frame. The new pixels overwrite the image's atlas region in place.
pub(crate) fn make_browser_frame_stream() -> Box<dyn FnMut()> {
    use schnellui::widgets::{DynamicImageFrame, Image};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    const W: u32 = 640;
    const H: u32 = 360;
    let frames: [Rc<[u8]>; 2] = [
        vec![0x20; (W * H * 4) as usize].into(),
        vec![0xd0; (W * H * 4) as usize].into(),
    ];
    let revision = Rc::new(Cell::new(0_u64));
    let current = Rc::new(RefCell::new(DynamicImageFrame::new(
        W,
        H,
        frames[0].clone(),
    )));
    let (source, feed) = (revision.clone(), current.clone());
    let view =
        Image::dynamic_rgba_versioned(move || source.get(), move || Some(feed.borrow().clone()));
    let mut app = App::mount_with_size(view, 800, 480);
    app.frame();
    let publish = move |app: &mut App| {
        let next = revision.get() + 1;
        current.borrow_mut().pixels = frames[(next % 2) as usize].clone();
        revision.set(next);
        app.frame();
    };
    for _ in 0..4 {
        publish(&mut app);
    }
    Box::new(move || publish(black_box(&mut app)))
}
