//! Paint emission cost and exactness: an edit touches the rows that can show,
//! never the document below them, and the spliced paint always equals a
//! from-scratch emission of the same state.

use super::*;
use crate::scene::{CompositeProperties, Rect};
use crate::widgets::{Column, EditKey, Flex, Scroll, TextPointerAction};
use crate::App;

fn touched() -> u64 {
    TOUCHED.with(|c| c.replace(0))
}

fn area_id(app: &App) -> WidgetId {
    app.widgets
        .with(|rt| rt.borrow().areas.keys().next())
        .expect("one text area")
}

fn with_area<R>(app: &App, f: impl FnOnce(&mut AreaState) -> R) -> R {
    let id = area_id(app);
    app.widgets
        .with(|rt| f(rt.borrow_mut().areas.get_mut(id).expect("area state")))
}

/// `lines` numbered lines with line 10 empty, in an 800x600 window.
fn tall_area(lines: usize, wrap: WrapMode) -> App {
    let source = (0..lines)
        .map(|i| {
            if i == 10 {
                String::new()
            } else {
                format!("{i:>5} the quick brown fox jumps")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut app = App::mount_with_size(
        Column::new().child(TextArea::new(source).wrap(wrap)),
        800,
        600,
    );
    app.frame();
    let id = area_id(&app);
    app.focus(Some(id));
    app.dispatch_edit_key(EditKey::DocStart { select: false });
    for _ in 0..10 {
        app.dispatch_edit_key(EditKey::Down { select: false });
    }
    app.frame();
    app
}

/// Primitives each keystroke wrote or moved, typing on line 10, plus whether
/// any keystroke changed line 10's row count.
fn type_on_line_ten(lines: usize, wrap: WrapMode) -> (Vec<u64>, bool) {
    let mut app = tall_area(lines, wrap);
    let rows = |app: &App| with_area(app, |st| st.cache.lines[10].rows);
    let mut counts = Vec::new();
    let mut step = |app: &mut App, key: EditKey<'_>| {
        touched();
        app.dispatch_edit_key(key);
        app.frame();
        counts.push(touched());
    };
    // the first character on an empty line: 0 -> 1 glyph primitives
    step(&mut app, EditKey::Insert("x"));
    step(&mut app, EditKey::Insert("y"));
    step(&mut app, EditKey::Backspace);
    let mut rewrapped = false;
    if wrap != WrapMode::NoWrap {
        // words until the line wraps into a second row
        for _ in 0..200 {
            let before = rows(&app);
            step(&mut app, EditKey::Insert("ab "));
            if rows(&app) != before {
                rewrapped = true;
                break;
            }
        }
    }
    step(&mut app, EditKey::Insert("z"));
    // a new line, and joining it back
    step(&mut app, EditKey::Enter);
    step(&mut app, EditKey::Backspace);
    // a multi-line paste
    step(&mut app, EditKey::Insert("one\ntwo\nthree"));
    (counts, rewrapped)
}

/// Typing on line 10 writes and moves the same primitives in a 1000-line and
/// a 20,000-line area: the rows below the window own no primitives, so the
/// splice never shifts the document's tail. Covers an edit that gives an
/// empty line its first glyph, a newline, a join, a paste and (wrapping) a
/// character that wraps the line into two rows.
#[test]
fn typing_on_line_ten_touches_the_same_primitives_at_any_document_length() {
    for wrap in [WrapMode::NoWrap, WrapMode::Word] {
        let (small, small_wrapped) = type_on_line_ten(1000, wrap);
        let (large, large_wrapped) = type_on_line_ten(20_000, wrap);
        assert_eq!(small_wrapped, large_wrapped);
        assert_eq!(
            wrap != WrapMode::NoWrap,
            large_wrapped,
            "{wrap:?}: a keystroke re-wrapped line 10"
        );
        assert_eq!(small, large, "{wrap:?}: primitives touched per keystroke");
        // a window of ~35 rows of ~30 glyphs, plus overscan: far below the
        // ~600,000 glyph primitives a 20,000-line paint would hold
        let worst = large.iter().copied().max().unwrap();
        assert!(
            worst < 6000,
            "{wrap:?}: {worst} primitives for one keystroke"
        );
    }
}

/// Primitive equality up to the float drift of rows slid by `shift_y`.
fn close(a: &Primitive, b: &Primitive) -> bool {
    let near = |x: f32, y: f32| (x - y).abs() < 1e-2;
    let rect = |a: &Rect, b: &Rect| {
        near(a.x, b.x) && near(a.y, b.y) && near(a.width, b.width) && near(a.height, b.height)
    };
    match (a, b) {
        (
            Primitive::SolidRect {
                rect: r1,
                color: c1,
                corner_radius: k1,
            },
            Primitive::SolidRect {
                rect: r2,
                color: c2,
                corner_radius: k2,
            },
        ) => rect(r1, r2) && c1 == c2 && k1 == k2,
        (
            Primitive::GlyphQuad {
                rect: r1,
                atlas_uv: u1,
                color: c1,
            },
            Primitive::GlyphQuad {
                rect: r2,
                atlas_uv: u2,
                color: c2,
            },
        ) => rect(r1, r2) && u1 == u2 && c1 == c2,
        (
            Primitive::Line {
                from: f1,
                to: t1,
                width: w1,
                color: c1,
            },
            Primitive::Line {
                from: f2,
                to: t2,
                width: w2,
                color: c2,
            },
        ) => {
            near(f1.x, f2.x)
                && near(f1.y, f2.y)
                && near(t1.x, t2.x)
                && near(t1.y, t2.y)
                && w1 == w2
                && c1 == c2
        }
        _ => a == b,
    }
}

/// The area's paint must equal a from-scratch emission of the same state (a
/// fresh line table, the same value, selections and band), and its band must
/// cover every row that can show. The retained state is restored afterwards.
fn assert_matches_from_scratch(app: &mut App, what: &str) {
    let id = area_id(app);
    let (end, band, content) = with_area(app, |st| {
        let end = st.cache.ranges.expect("emitted").caret_end;
        (end, st.cache.band, st.content)
    });
    let saved = app.scene.paint(id).unwrap().primitives.clone();

    // the band covers what the window or the viewport shows
    let (rect, line_h, empty) = with_area(app, |st| {
        (
            st.last_rect.unwrap(),
            st.cache.line_h.unwrap().1,
            st.value.is_empty(),
        )
    });
    let window = app.widgets.with(|rt| rt.borrow().window);
    let visible = visible_rows(&app.scene, id, rect.y + AREA_PAD, line_h, window);
    if !empty {
        assert_eq!(visible.is_some(), band.is_some(), "{what}: band presence");
        let rows = visible.map(|v| match v {
            Visible::Scroll(lo, hi) | Visible::Window { lo, hi, .. } => (lo, hi),
        });
        if let (Some((lo, hi)), Some((b0, b1))) = (rows, band) {
            assert!(
                b0 <= lo && hi <= b1,
                "{what}: band {b0}..{b1} misses rows {lo}..{hi}"
            );
        }
    }

    // A Scroll keeps its band while it covers the viewport, so the fresh
    // emission starts from the same one. A window band depends on what shows
    // alone: the fresh emission must arrive at it by itself.
    let in_scroll = matches!(visible, Some(Visible::Scroll(..)));
    let old = with_area(app, |st| {
        let mut fresh = AreaCache::new(&st.value);
        fresh.band = band.filter(|_| in_scroll);
        std::mem::replace(&mut st.cache, fresh)
    });
    emit_text_area_paint(
        &app.widgets,
        &mut app.scene,
        &mut app.text,
        &mut app.atlas,
        id,
    );
    let (fresh_end, fresh_content) =
        with_area(app, |st| (st.cache.ranges.unwrap().caret_end, st.content));
    let got = &saved[..end];
    let want = &app.scene.paint(id).unwrap().primitives[..fresh_end];
    let fresh_band = with_area(app, |st| st.cache.band);
    assert_eq!(band, fresh_band, "{what}: band");
    assert_eq!(content, fresh_content, "{what}: content size");
    assert_eq!(got.len(), want.len(), "{what}: primitive count");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert!(
            close(g, w),
            "{what}: primitive {i} differs\n got  {g:?}\n want {w:?}"
        );
    }
    with_area(app, |st| st.cache = old);
    app.scene.paint_mut(id).primitives = saved;
}

/// xorshift64*: deterministic randomness without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn coin(&mut self) -> bool {
        self.next() & 1 == 1
    }
}

fn keyword_highlighter(src: &str) -> Vec<Vec<RichSpan>> {
    use crate::widgets::rich::SpanStyle;
    src.split('\n')
        .map(|line| match line.strip_prefix("fn") {
            Some(rest) => vec![
                RichSpan::new(
                    "fn",
                    SpanStyle {
                        bold: true,
                        ..SpanStyle::token(Color::rgb(0x33, 0x99, 0x55))
                    },
                ),
                RichSpan::code(rest),
            ],
            None => vec![RichSpan::code(line)],
        })
        .collect()
}

/// Random edits, selection changes, scrolls, compositor slides and resizes;
/// after every step the spliced paint equals a from-scratch emission.
#[test]
fn spliced_paint_matches_from_scratch_under_random_edits() {
    // (wrap, inside a Scroll, gutter, highlighter)
    let configs = [
        (WrapMode::NoWrap, false, true, false),
        (WrapMode::NoWrap, true, false, true),
        (WrapMode::Word, false, false, true),
        (WrapMode::Word, true, true, false),
        (WrapMode::Anywhere, false, true, true),
    ];
    let words = [
        "fn",
        "x",
        "let",
        "quick",
        "a b",
        "",
        "wide_token_that_never_breaks",
    ];
    for (seed, &(wrap, scrolled, gutter, highlight)) in configs.iter().enumerate() {
        let source = (0..120)
            .map(|i| match i % 7 {
                0 => format!("fn item_{i}() {{"),
                3 => String::new(),
                5 => "word ".repeat(i % 30),
                _ => format!("    let v{i} = {i};"),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut view = TextArea::new(source).wrap(wrap);
        if gutter {
            view = view.line_numbers(1, 999, Color::rgb(1, 2, 3));
        }
        if highlight {
            view = view.highlight(keyword_highlighter);
        }
        let root = if scrolled {
            Column::new().child(
                Scroll::new()
                    .label("area scroll")
                    .size(500.0, 260.0)
                    .child(Flex::new().shrink(0.0).child(view)),
            )
        } else {
            Column::new().child(view)
        };
        let mut app = App::mount_with_size(root, 640, 300);
        app.frame();
        let id = area_id(&app);
        app.focus(Some(id));
        app.frame();
        let scroll = app.find_widget(crate::a11y::Role::ScrollView, Some("area scroll"));
        let column = app.scene.root().unwrap();
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (seed as u64 + 1));
        assert_matches_from_scratch(&mut app, "mount");
        for step in 0..160 {
            let op = rng.below(19);
            let select = rng.coin();
            match op {
                0..=3 => {
                    let w = words[rng.below(words.len() as u64) as usize];
                    app.dispatch_edit_key(EditKey::Insert(w));
                }
                4 => {
                    app.dispatch_edit_key(EditKey::Insert("paste one\nfn two\n\nthree"));
                }
                5 => {
                    app.dispatch_edit_key(EditKey::Enter);
                }
                6 | 7 => {
                    app.dispatch_edit_key(EditKey::Backspace);
                }
                8 => {
                    app.dispatch_edit_key(EditKey::Delete);
                }
                9 => {
                    for _ in 0..rng.below(12) {
                        app.dispatch_edit_key(EditKey::Down { select });
                    }
                }
                10 => {
                    for _ in 0..rng.below(12) {
                        app.dispatch_edit_key(EditKey::Up { select });
                    }
                }
                11 => {
                    let word = rng.coin();
                    app.dispatch_edit_key(EditKey::Right { select, word });
                    app.dispatch_edit_key(EditKey::Left {
                        select: false,
                        word,
                    });
                    app.dispatch_edit_key(EditKey::Right { select, word });
                }
                12 => {
                    app.dispatch_edit_key(if rng.coin() {
                        EditKey::Home { select }
                    } else {
                        EditKey::End { select }
                    });
                }
                13 => {
                    // an extra caret somewhere in the box
                    let rect = with_area(&app, |st| st.last_rect.unwrap());
                    let p = Point {
                        x: rect.x + 30.0 + rng.below(200) as f32,
                        y: rect.y + 8.0 + rng.below(250) as f32,
                    };
                    let (w, s, t, a) =
                        (&app.widgets, &mut app.scene, &mut app.text, &mut app.atlas);
                    crate::widgets::dispatch_text_pointer_action(
                        w,
                        s,
                        t,
                        a,
                        id,
                        p,
                        TextPointerAction::AddCaret,
                    );
                }
                14 => {
                    // scroll the viewport, or slide the whole column on the compositor
                    if let Some(sv) = scroll {
                        let y = rng.below(1800) as f32;
                        app.scene.set_scroll_offset(sv, Point { x: 0.0, y });
                    } else {
                        let y = -(rng.below(1500) as f32);
                        app.scene.set_composite_properties(
                            column,
                            CompositeProperties {
                                translation: Point { x: 0.0, y },
                                ..CompositeProperties::default()
                            },
                        );
                    }
                }
                15 => {
                    // resize: a new window band, and a rewrap when wrapping
                    let w = 360.0 + rng.below(400) as f32;
                    let h = 160.0 + rng.below(400) as f32;
                    app.resize(w, h);
                }
                16 => {
                    app.dispatch_edit_key(if rng.coin() {
                        EditKey::Undo
                    } else {
                        EditKey::Redo
                    });
                }
                17 => {
                    if rng.below(4) == 0 {
                        // wash every row in the band, then collapse again so
                        // the document keeps its length
                        app.dispatch_edit_key(EditKey::SelectAll);
                        app.frame();
                        assert_matches_from_scratch(&mut app, "select all");
                        app.dispatch_edit_key(EditKey::Right {
                            select: false,
                            word: false,
                        });
                    } else {
                        app.dispatch_edit_key(EditKey::DocEnd { select });
                    }
                }
                _ => {
                    app.dispatch_edit_key(EditKey::DocStart { select });
                }
            }
            app.frame();
            assert_matches_from_scratch(
                &mut app,
                &format!(
                    "{wrap:?}/scroll {scrolled}/gutter {gutter}/hl {highlight} step {step} op {op}"
                ),
            );
        }
    }
}
