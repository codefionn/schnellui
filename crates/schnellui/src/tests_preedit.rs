//! Input-method composition in the built-in text fields. A preedit is drawn
//! inline at the caret, underlined, and changes nothing else: not the value,
//! not the accessible value, not the change handlers, not the undo history.
//! The oracle for "drawn inline" is a field whose value really holds the
//! composed text, so every position and wrap decision is checked against
//! the shaper and not against a formula.

use super::*;
use crate::RawImeEvent;
use schnellui::a11y::Role;
use schnellui::scene::{Color, Point, Primitive, Rect};
use schnellui::widgets::{
    Column, ComponentInteractions, InteractionStates, InteractionStyle, TextArea, TextInput, Theme,
    View, WrapMode, DEFAULT_THEME,
};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Input,
    Area,
    Wrapped,
}
const KINDS: [Kind; 3] = [Kind::Input, Kind::Area, Kind::Wrapped];

fn view(kind: Kind, value: &str, seen: Rc<RefCell<Vec<String>>>) -> Box<dyn View> {
    let on_input = move |v: &str| seen.borrow_mut().push(v.to_owned());
    match kind {
        Kind::Input => Box::new(TextInput::new(value).on_input(on_input)),
        Kind::Area => Box::new(
            TextArea::new(value)
                .wrap(WrapMode::NoWrap)
                .on_input(on_input),
        ),
        Kind::Wrapped => Box::new(
            TextArea::new(value)
                .rows(1)
                .wrap(WrapMode::Word)
                .on_input(on_input),
        ),
    }
}

fn role(kind: Kind) -> Role {
    match kind {
        Kind::Input => Role::TextInput,
        _ => Role::MultilineTextInput,
    }
}

struct Rig {
    app: App,
    id: scene::WidgetId,
    kind: Kind,
    seen: Rc<RefCell<Vec<String>>>,
}

fn rig_in(kind: Kind, value: &str, width: u32, theme: Theme) -> Rig {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let mut app = App::mount_with_theme_size_scaled(
        theme,
        Column::new().child(view(kind, value, seen.clone())),
        width,
        240,
        1.0,
    );
    app.apply_reduced_motion(true);
    app.frame();
    let id = app.find_widget(role(kind), None).unwrap();
    app.focus(Some(id));
    app.frame();
    Rig {
        app,
        id,
        kind,
        seen,
    }
}

fn rig(kind: Kind, value: &str) -> Rig {
    rig_in(kind, value, 400, DEFAULT_THEME)
}

impl Rig {
    fn preedit(&mut self, text: &str, cursor: Option<(usize, usize)>) {
        self.app.dispatch_ime(RawImeEvent::Preedit {
            text: text.into(),
            cursor,
        });
        self.app.frame();
    }
    fn commit(&mut self, text: &str) {
        self.app.dispatch_ime(RawImeEvent::Commit(text.into()));
        self.app.frame();
    }
    fn value(&self) -> String {
        self.app
            .scene()
            .a11y(self.id)
            .unwrap()
            .value
            .clone()
            .unwrap()
    }
    fn prims(&self) -> Vec<Primitive> {
        self.app.scene().paint(self.id).unwrap().primitives.clone()
    }
    /// The glyph boxes. A single-line field is measured once at build, so
    /// its vertical placement follows the value it was built with: only the
    /// inline positions are comparable there.
    fn glyphs(&self) -> Vec<Rect> {
        let mut rects = glyph_rects(&self.prims());
        if self.kind == Kind::Input {
            for r in &mut rects {
                r.y = 0.0;
                r.height = 0.0;
            }
        }
        rects
    }
    fn key(&mut self, key: UiKey<'_>) {
        self.app.dispatch_key(key);
        self.app.frame();
    }
    fn select_back(&mut self, n: usize) {
        for _ in 0..n {
            self.key(UiKey::Left {
                shift: true,
                ctrl: false,
            });
        }
    }
}

fn glyph_rects(prims: &[Primitive]) -> Vec<Rect> {
    prims
        .iter()
        .filter_map(|p| match p {
            Primitive::GlyphQuad { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect()
}

/// The x of the caret line, if the paint has exactly one.
fn caret_x(prims: &[Primitive]) -> Option<f32> {
    let lines: Vec<_> = prims
        .iter()
        .filter_map(|p| match p {
            Primitive::Line { from, to, .. } if from.x == to.x => Some((from.x, from.y)),
            _ => None,
        })
        .collect();
    (lines.len() == 1).then(|| lines[0].0)
}

fn caret_pos(prims: &[Primitive]) -> Option<(f32, f32)> {
    prims.iter().find_map(|p| match p {
        Primitive::Line { from, to, .. } if from.x == to.x => Some((from.x, from.y)),
        _ => None,
    })
}

/// Thin solid rects: the underline segments.
fn underlines(prims: &[Primitive]) -> Vec<Rect> {
    prims
        .iter()
        .filter_map(|p| match p {
            Primitive::SolidRect { rect, color, .. }
                if rect.height <= 3.0 && rect.width > 0.0 && *color == DEFAULT_THEME.text =>
            {
                Some(*rect)
            }
            _ => None,
        })
        .collect()
}

/// A rig whose value really is `composed`, caret at the end.
fn composed(kind: Kind, value: &str) -> Rig {
    rig(kind, value)
}

#[test]
fn a_preedit_draws_inline_underlined_and_changes_nothing_else() {
    for kind in KINDS {
        let mut f = rig(kind, "ab");
        let base_glyphs = f.glyphs().len();
        assert!(underlines(&f.prims()).is_empty(), "{kind:?}");
        f.preedit("にほ", Some((6, 6)));
        let oracle = composed(kind, "abにほ");
        assert_eq!(f.glyphs(), oracle.glyphs(), "{kind:?}: glyphs sit inline");
        assert!(f.glyphs().len() > base_glyphs, "{kind:?}");
        let rules = underlines(&f.prims());
        assert_eq!(rules.len(), 1, "{kind:?}: one underline");
        // The underline covers the preedit glyphs, not the text before it.
        let preedit_glyphs = &f.glyphs()[2..];
        let left = preedit_glyphs.iter().map(|g| g.x).fold(f32::MAX, f32::min);
        let right = preedit_glyphs
            .iter()
            .map(|g| g.x + g.width)
            .fold(f32::MIN, f32::max);
        assert!(rules[0].x <= left + 1.5 && rules[0].x + rules[0].width >= right - 1.5);
        assert!(rules[0].x >= f.glyphs()[1].x, "{kind:?}: not under `ab`");
        assert_eq!(f.value(), "ab", "{kind:?}: a11y value untouched");
        assert!(f.seen.borrow().is_empty(), "{kind:?}: no change handler");
        assert!(caret_x(&f.prims()).is_some(), "{kind:?}: the caret stays");
    }
}

#[test]
fn the_preedit_cursor_places_the_caret() {
    for kind in KINDS {
        let mut f = rig(kind, "ab");
        f.preedit("にほ", Some((3, 3)));
        let oracle = composed(kind, "abに");
        assert_eq!(
            caret_x(&f.prims()),
            caret_x(&oracle.prims()),
            "{kind:?}: caret after the first composed character"
        );
        f.preedit("にほ", Some((0, 0)));
        let oracle = composed(kind, "ab");
        assert_eq!(caret_x(&f.prims()), caret_x(&oracle.prims()), "{kind:?}");
    }
}

#[test]
fn a_cursor_range_is_a_thicker_underline_and_no_caret() {
    for kind in KINDS {
        let mut f = rig(kind, "ab");
        f.preedit("にほ", Some((0, 3)));
        let rules = underlines(&f.prims());
        assert_eq!(rules.len(), 2, "{kind:?}: preedit rule and cursor segment");
        let thick = rules.iter().map(|r| r.height).fold(0.0, f32::max);
        let thin = rules.iter().map(|r| r.height).fold(f32::MAX, f32::min);
        assert!(thick > thin, "{kind:?}");
        assert_eq!(caret_x(&f.prims()), None, "{kind:?}: no caret for a range");
        f.preedit("にほ", None);
        assert_eq!(caret_x(&f.prims()), None, "{kind:?}: None hides the caret");
        assert_eq!(underlines(&f.prims()).len(), 1, "{kind:?}");
    }
}

#[test]
fn a_preedit_replaces_the_selection_visually_only() {
    for kind in KINDS {
        let mut f = rig(kind, "hello world");
        f.select_back(5);
        f.preedit("にほ", Some((6, 6)));
        let oracle = composed(kind, "hello にほ");
        assert_eq!(f.glyphs(), oracle.glyphs(), "{kind:?}");
        assert_eq!(f.value(), "hello world", "{kind:?}");
        // the selection wash is gone while it is replaced
        let wash = f.prims().iter().any(|p| {
            matches!(p, Primitive::SolidRect { color, .. } if *color == DEFAULT_THEME.text_selection)
        });
        assert!(!wash, "{kind:?}: no wash under the preedit");
        // cancelling brings the selection back
        f.preedit("", None);
        let wash = f.prims().iter().any(|p| {
            matches!(p, Primitive::SolidRect { color, .. } if *color == DEFAULT_THEME.text_selection)
        });
        assert!(wash, "{kind:?}: selection returns");
        assert_eq!(f.app.selected_text().as_deref(), Some("world"), "{kind:?}");
        // committing replaces it
        f.preedit("にほ", Some((6, 6)));
        f.commit("日本");
        assert_eq!(f.value(), "hello 日本", "{kind:?}");
        assert_eq!(f.seen.borrow().as_slice(), ["hello 日本"], "{kind:?}");
    }
}

#[test]
fn a_commit_after_a_preedit_is_one_undo_step() {
    for kind in KINDS {
        let mut f = rig(kind, "ab");
        f.preedit("に", Some((3, 3)));
        f.preedit("にほ", Some((6, 6)));
        assert_eq!(underlines(&f.prims()).len(), 1, "{kind:?}: composing");
        assert!(f.seen.borrow().is_empty());
        f.preedit("", None);
        f.commit("日本");
        assert_eq!(f.value(), "ab日本", "{kind:?}");
        assert!(underlines(&f.prims()).is_empty(), "{kind:?}");
        assert_eq!(f.glyphs(), composed(kind, "ab日本").glyphs(), "{kind:?}");
        f.key(UiKey::Undo);
        assert_eq!(f.value(), "ab", "{kind:?}: one step takes the commit back");
        assert!(!f.app.dispatch_key(UiKey::Undo), "{kind:?}: and no more");
    }
}

#[test]
fn an_empty_preedit_or_disabled_clears_it() {
    for kind in KINDS {
        let mut f = rig(kind, "ab");
        let before = f.prims();
        f.preedit("にほ", Some((6, 6)));
        assert_ne!(f.prims(), before);
        f.preedit("", None);
        assert_eq!(f.prims(), before, "{kind:?}: empty preedit");
        f.preedit("にほ", Some((6, 6)));
        f.app.dispatch_ime(RawImeEvent::Disabled);
        f.app.frame();
        assert_eq!(f.prims(), before, "{kind:?}: Disabled");
        assert_eq!(f.value(), "ab");
    }
}

#[test]
fn blur_clears_the_preedit() {
    for kind in KINDS {
        let mut f = rig(kind, "ab");
        f.app.focus(None);
        f.app.frame();
        let blurred = f.prims();
        f.app.focus(Some(f.id));
        f.preedit("にほ", Some((6, 6)));
        assert_eq!(f.glyphs().len(), 4, "{kind:?}: composing");
        f.app.focus(None);
        f.app.frame();
        assert_eq!(f.prims(), blurred, "{kind:?}: nothing of it is left");
        f.app.focus(Some(f.id));
        f.app.frame();
        assert_eq!(f.glyphs(), composed(kind, "ab").glyphs(), "{kind:?}");
        assert!(underlines(&f.prims()).is_empty());
    }
}

#[test]
fn a_key_or_a_click_ends_the_preedit_before_it_acts() {
    for kind in KINDS {
        let mut f = rig(kind, "ab");
        f.preedit("にほ", Some((6, 6)));
        assert_eq!(underlines(&f.prims()).len(), 1, "{kind:?}: composing");
        f.key(UiKey::Char("x"));
        assert_eq!(f.value(), "abx", "{kind:?}: typed into the real value");
        assert!(underlines(&f.prims()).is_empty(), "{kind:?}");
        f.preedit("にほ", Some((6, 6)));
        f.key(UiKey::Left {
            shift: false,
            ctrl: false,
        });
        assert!(
            underlines(&f.prims()).is_empty(),
            "{kind:?}: a move ends it"
        );
        assert_eq!(f.glyphs(), composed(kind, "abx").glyphs(), "{kind:?}");
    }
}

#[test]
fn a_preedit_wraps_a_text_area_row_at_the_right_edge() {
    let text = "こんにちは世界のみなさん";
    let mut f = rig_in(Kind::Wrapped, "word word word", 190, DEFAULT_THEME);
    let rows_before = {
        let ys: std::collections::BTreeSet<i32> = f.glyphs().iter().map(|g| g.y as i32).collect();
        ys.len()
    };
    f.preedit(text, Some((text.len(), text.len())));
    let oracle = rig_in(
        Kind::Wrapped,
        &format!("word word word{text}"),
        190,
        DEFAULT_THEME,
    );
    assert_eq!(
        f.glyphs(),
        oracle.glyphs(),
        "wraps where a real value would"
    );
    let ys: std::collections::BTreeSet<i32> = f.glyphs().iter().map(|g| g.y as i32).collect();
    assert!(ys.len() > rows_before, "the preedit made a new row");
    // the underline follows onto every row it covers
    let rules = underlines(&f.prims());
    assert!(rules.len() >= 2, "one rule per row: {rules:?}");
    // the box grew with it
    let grown = f.app.scene().layout(f.id).unwrap().rect.height;
    assert_eq!(
        grown,
        oracle.app.scene().layout(oracle.id).unwrap().rect.height
    );
    f.preedit("", None);
    assert_eq!(f.value(), "word word word");
    let shrunk = f.app.scene().layout(f.id).unwrap().rect.height;
    assert!(shrunk < grown, "the box shrinks back");
}

fn layered_theme() -> Theme {
    let style = |r: u8| InteractionStyle {
        background: Some(Color::rgba(r, 40, 90, 0x50)),
        foreground: Some(Color::rgb(r, 10, 200)),
        border: Some(Color::rgb(r, 120, 30)),
    };
    let states = |r: u8| InteractionStates {
        hover: style(r),
        focus: style(r + 20),
        active: style(r + 40),
    };
    Theme {
        interactions: states(10),
        component_interactions: ComponentInteractions {
            editable: Some(states(190)),
            ..ComponentInteractions::default()
        },
        ..DEFAULT_THEME
    }
}

fn centre(app: &App, id: scene::WidgetId) -> Point {
    let r = app.scene().layout(id).unwrap().rect;
    Point {
        x: r.x + r.width / 2.0,
        y: r.y + r.height / 2.0,
    }
}

const AWAY: Point = Point {
    x: f32::NEG_INFINITY,
    y: f32::NEG_INFINITY,
};

#[test]
fn a_preedit_under_hover_leaves_no_stale_decoration_when_the_pointer_goes() {
    for kind in KINDS {
        // the same gestures without a preedit
        let mut plain = rig_in(kind, "ab", 400, layered_theme());
        let p = centre(&plain.app, plain.id);
        plain.app.update_pointer_proximity(p);
        plain.app.frame();
        plain.app.update_pointer_proximity(AWAY);
        plain.app.frame();

        let mut f = rig_in(kind, "ab", 400, layered_theme());
        let p = centre(&f.app, f.id);
        f.app.update_pointer_proximity(p);
        f.app.frame();
        let hovered = f.prims();
        f.preedit("にほ", Some((6, 6)));
        f.preedit("にほん", Some((9, 9)));
        assert_ne!(f.prims().len(), hovered.len(), "{kind:?}");
        // hover colours survive the repaint: the hovered preedit differs from
        // an unhovered one in more than the underline
        let mut calm = rig_in(kind, "ab", 400, layered_theme());
        calm.preedit("にほ", Some((6, 6)));
        calm.preedit("にほん", Some((9, 9)));
        assert_ne!(f.prims(), calm.prims(), "{kind:?}: hover still painted");
        f.app.update_pointer_proximity(AWAY);
        f.app.frame();
        assert_eq!(
            f.prims(),
            calm.prims(),
            "{kind:?}: hover gone, nothing stale"
        );
        f.preedit("", None);
        assert_eq!(
            f.prims(),
            plain.prims(),
            "{kind:?}: back to the plain paint"
        );
    }
}

#[test]
fn the_ime_area_follows_the_preedit_cursor() {
    for kind in KINDS {
        let mut f = rig(kind, "ab");
        let field = f.app.ime_area().unwrap();
        f.preedit("にほ", Some((3, 3)));
        let area = f.app.ime_area().unwrap();
        let (x, y) = caret_pos(&f.prims()).unwrap();
        assert!(
            area.width < field.width && area.height < field.height,
            "{kind:?}"
        );
        assert!((area.x - x).abs() <= 2.0, "{kind:?}: {area:?} at caret {x}");
        assert!(area.y <= y + 1.0 && area.y + area.height >= y, "{kind:?}");
        f.preedit("にほ", Some((6, 6)));
        let moved = f.app.ime_area().unwrap();
        assert!(moved.x > area.x, "{kind:?}: follows the cursor");
        f.preedit("", None);
        assert_eq!(f.app.ime_area(), Some(field), "{kind:?}: the field again");
    }
}

#[test]
fn a_scrolled_field_reveals_the_preedit_cursor() {
    let long = "the quick brown fox jumps over the lazy dog and keeps running";
    let mut f = rig_in(Kind::Input, long, 200, DEFAULT_THEME);
    let field = f.app.scene().layout(f.id).unwrap().rect;
    f.preedit("にほんごにほんご", Some((24, 24)));
    let (x, _) = caret_pos(&f.prims()).unwrap();
    assert!(
        x >= field.x && x <= field.x + field.width,
        "caret {x} inside {field:?}"
    );
    let rules = underlines(&f.prims());
    assert_eq!(rules.len(), 1);
    assert!(rules[0].x + rules[0].width <= field.x + field.width + 0.5);
}

#[test]
fn a_remount_clears_the_preedit_and_keeps_the_value() {
    for kind in KINDS {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut app = App::mount_themed(theme::LIGHT, {
            let seen = seen.clone();
            move || Column::new().child(view(kind, "ab", seen.clone()))
        });
        app.frame();
        let id = app.find_widget(role(kind), None).unwrap();
        app.focus(Some(id));
        let mut f = Rig {
            app,
            id,
            kind,
            seen,
        };
        f.preedit("にほ", Some((6, 6)));
        assert_eq!(f.glyphs().len(), 4, "{kind:?}: composing");
        assert!(f.app.set_theme(theme::DARK));
        f.app.frame();
        let id = f.app.find_widget(role(kind), None).unwrap();
        let prims = f.app.scene().paint(id).unwrap().primitives.clone();
        assert_eq!(
            f.app.scene().a11y(id).unwrap().value.as_deref(),
            Some("ab"),
            "{kind:?}"
        );
        assert_eq!(glyph_rects(&prims).len(), 2, "{kind:?}: only `ab` is drawn");
    }
}

/// The underline reaches the screen: the pixel under the middle of the
/// composed text is ink, and it was the field's surface before. Skips when
/// there is no GPU adapter (SOUL §7.2).
#[test]
fn the_underline_is_ink_on_screen() {
    for kind in KINDS {
        let mut f = rig(kind, "ab");
        let Some(before) = f.app.render_rgba8() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        f.preedit("にほ", Some((6, 6)));
        let rule = underlines(&f.prims())[0];
        let after = f.app.render_rgba8().unwrap();
        let w = (f.app.size().width * f.app.scale()).round() as usize;
        let at = |rgba: &[u8]| {
            let (x, y) = (
                (rule.x + rule.width / 2.0) as usize,
                (rule.y + 0.5) as usize,
            );
            let i = (y * w + x) * 4;
            [rgba[i], rgba[i + 1], rgba[i + 2]]
        };
        assert_eq!(at(&before), [255, 255, 255], "{kind:?}: surface before");
        let ink = DEFAULT_THEME.text;
        let dist = |c: [u8; 3]| {
            (i32::from(c[0]) - i32::from(ink.r)).abs()
                + (i32::from(c[1]) - i32::from(ink.g)).abs()
                + (i32::from(c[2]) - i32::from(ink.b)).abs()
        };
        assert!(
            dist(at(&after)) < 40,
            "{kind:?}: {:?} is not ink",
            at(&after)
        );
    }
}

/// A protected value stays protected while it is composed into: the composed
/// text is drawn as bullets, like what is typed.
#[test]
fn a_password_field_shows_a_preedit_as_bullets() {
    let mount = |value: &str| {
        let mut app = App::mount(schnellui::widgets::PasswordInput::new(value));
        app.frame();
        let id = app.find_widget(Role::PasswordInput, None).unwrap();
        app.focus(Some(id));
        app.frame();
        (app, id)
    };
    let xs = |app: &App, id| {
        glyph_rects(&app.scene().paint(id).unwrap().primitives)
            .iter()
            .map(|r| (r.x, r.width))
            .collect::<Vec<_>>()
    };
    let (mut app, id) = mount("ab");
    app.dispatch_ime(RawImeEvent::Preedit {
        text: "にほ".into(),
        cursor: Some((6, 6)),
    });
    app.frame();
    let (oracle, oracle_id) = mount("abcd");
    assert_eq!(xs(&app, id), xs(&oracle, oracle_id));
    assert_eq!(
        app.scene().a11y(id).unwrap().value.as_deref(),
        Some("••"),
        "the accessible value does not move"
    );
}
